import SwiftUI
import AVFoundation

#if canImport(UIKit)
import UIKit

struct QRScannerView: View {
    @Binding var isPresented: Bool
    let onScanned: (String, UInt16, String) -> Void

    @State private var cameraPermissionGranted = false
    @State private var showPermissionDenied = false

    var body: some View {
        NavigationStack {
            ZStack {
                JC.Colors.background.ignoresSafeArea()

                if cameraPermissionGranted {
                    QRCameraView { uri in
                        if let parsed = parseJCodeURI(uri) {
                            onScanned(parsed.host, parsed.port, parsed.code)
                            isPresented = false
                        }
                    }
                    .ignoresSafeArea()
                    .overlay(alignment: .bottom) {
                        VStack(spacing: JC.Spacing.sm) {
                            Image(systemName: "viewfinder")
                                .font(.system(size: 24))
                                .foregroundStyle(JC.Colors.accent)
                            Text("Point at the QR code from **jcode pair**")
                                .font(JC.Fonts.callout)
                                .foregroundStyle(JC.Colors.textPrimary)
                        }
                        .padding(JC.Spacing.lg)
                        .background(.ultraThinMaterial)
                        .clipShape(RoundedRectangle(cornerRadius: JC.Radius.md))
                        .padding(.bottom, 40)
                    }
                } else if showPermissionDenied {
                    VStack(spacing: JC.Spacing.lg) {
                        Image(systemName: "camera.fill")
                            .font(.system(size: 40))
                            .foregroundStyle(JC.Colors.textTertiary)
                        Text("Camera Access Required")
                            .font(JC.Fonts.title2)
                            .foregroundStyle(JC.Colors.textPrimary)
                        Text("Grant camera access in Settings to scan QR codes.")
                            .font(JC.Fonts.callout)
                            .foregroundStyle(JC.Colors.textSecondary)
                            .multilineTextAlignment(.center)
                    }
                    .padding(JC.Spacing.xxl)
                } else {
                    VStack(spacing: JC.Spacing.md) {
                        ProgressView()
                            .tint(JC.Colors.accent)
                        Text("Requesting camera access...")
                            .font(JC.Fonts.callout)
                            .foregroundStyle(JC.Colors.textSecondary)
                    }
                }
            }
            .navigationTitle("Scan QR Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { isPresented = false }
                        .foregroundStyle(JC.Colors.textSecondary)
                }
            }
        }
        .presentationBackground(JC.Colors.background)
        .task {
            await requestCameraAccess()
        }
    }

    private func requestCameraAccess() async {
        let status = AVCaptureDevice.authorizationStatus(for: .video)
        switch status {
        case .authorized:
            cameraPermissionGranted = true
        case .notDetermined:
            let granted = await AVCaptureDevice.requestAccess(for: .video)
            cameraPermissionGranted = granted
            showPermissionDenied = !granted
        default:
            showPermissionDenied = true
        }
    }

    private func parseJCodeURI(_ string: String) -> (host: String, port: UInt16, code: String)? {
        guard let url = URL(string: string),
              url.scheme == "jcode",
              url.host == "pair",
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let items = components.queryItems else {
            return nil
        }

        let host = items.first(where: { $0.name == "host" })?.value
        let portStr = items.first(where: { $0.name == "port" })?.value
        let code = items.first(where: { $0.name == "code" })?.value

        guard let host, !host.isEmpty,
              let portStr, let port = UInt16(portStr),
              let code, !code.isEmpty else {
            return nil
        }

        return (host, port, code)
    }
}

struct QRCameraView: UIViewControllerRepresentable {
    let onCodeScanned: (String) -> Void

    func makeUIViewController(context: Context) -> QRScannerController {
        let controller = QRScannerController()
        controller.onCodeScanned = onCodeScanned
        return controller
    }

    func updateUIViewController(_ uiViewController: QRScannerController, context: Context) {}
}

private final class CaptureSessionWrapper: @unchecked Sendable {
    let session = AVCaptureSession()
    let queue = DispatchQueue(label: "com.jcode.mobile.qr.capture-session")
    private var configured = false

    func configure(delegate: AVCaptureMetadataOutputObjectsDelegate) {
        queue.async { [weak self, weak delegate] in
            guard let self, let delegate, !self.configured else { return }
            self.session.beginConfiguration()
            defer { self.session.commitConfiguration() }

            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  self.session.canAddInput(input) else {
                return
            }
            self.session.addInput(input)

            let output = AVCaptureMetadataOutput()
            guard self.session.canAddOutput(output) else { return }
            self.session.addOutput(output)
            output.setMetadataObjectsDelegate(delegate, queue: self.queue)
            if output.availableMetadataObjectTypes.contains(.qr) {
                output.metadataObjectTypes = [.qr]
            }
            self.configured = true
        }
    }

    func start() {
        queue.async { [weak self] in
            guard let self, !self.session.isRunning else { return }
            self.session.startRunning()
        }
    }

    func stop() {
        queue.async { [weak self] in
            guard let self, self.session.isRunning else { return }
            self.session.stopRunning()
        }
    }
}

final class QRScannerController: UIViewController {
    var onCodeScanned: ((String) -> Void)?
    private let wrapper = CaptureSessionWrapper()
    private let delegateHandler = MetadataDelegate()
    private var previewLayer: AVCaptureVideoPreviewLayer?

    override func viewDidLoad() {
        super.viewDidLoad()

        delegateHandler.onDetected = { [weak self] value in
            DispatchQueue.main.async {
                self?.handleDetection(value)
            }
        }
        wrapper.configure(delegate: delegateHandler)

        let previewLayer = AVCaptureVideoPreviewLayer(session: wrapper.session)
        previewLayer.frame = view.layer.bounds
        previewLayer.videoGravity = .resizeAspectFill
        view.layer.addSublayer(previewLayer)
        self.previewLayer = previewLayer
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.layer.bounds
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        wrapper.start()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        wrapper.stop()
    }

    private func handleDetection(_ value: String) {
        wrapper.stop()
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        onCodeScanned?(value)
    }
}

private final class MetadataDelegate: NSObject, AVCaptureMetadataOutputObjectsDelegate {
    var onDetected: ((String) -> Void)?
    private var fired = false

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !fired,
              let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              let value = object.stringValue,
              value.hasPrefix("jcode://") else {
            return
        }
        fired = true
        onDetected?(value)
    }
}
#endif
