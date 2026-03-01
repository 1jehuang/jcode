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
            Group {
                if cameraPermissionGranted {
                    QRCameraView { uri in
                        if let parsed = parseJCodeURI(uri) {
                            onScanned(parsed.host, parsed.port, parsed.code)
                            isPresented = false
                        }
                    }
                    .ignoresSafeArea()
                    .overlay(alignment: .bottom) {
                        Text("Point at the QR code from **jcode pair**")
                            .font(.subheadline)
                            .padding(12)
                            .background(.ultraThinMaterial)
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                            .padding(.bottom, 40)
                    }
                } else if showPermissionDenied {
                    ContentUnavailableView(
                        "Camera Access Required",
                        systemImage: "camera.fill",
                        description: Text("Grant camera access in Settings to scan QR codes.")
                    )
                } else {
                    ProgressView("Requesting camera access...")
                }
            }
            .navigationTitle("Scan QR Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { isPresented = false }
                }
            }
        }
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

final class QRScannerController: UIViewController, @preconcurrency AVCaptureMetadataOutputObjectsDelegate {
    var onCodeScanned: ((String) -> Void)?
    private let captureSession = AVCaptureSession()
    private var hasScanned = false

    override func viewDidLoad() {
        super.viewDidLoad()

        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device) else {
            return
        }

        captureSession.addInput(input)

        let output = AVCaptureMetadataOutput()
        captureSession.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let previewLayer = AVCaptureVideoPreviewLayer(session: captureSession)
        previewLayer.frame = view.layer.bounds
        previewLayer.videoGravity = .resizeAspectFill
        view.layer.addSublayer(previewLayer)

        let session = captureSession
        Task.detached {
            session.startRunning()
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        let session = captureSession
        Task.detached {
            session.stopRunning()
        }
    }

    nonisolated func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              let value = object.stringValue,
              value.hasPrefix("jcode://") else {
            return
        }

        let callback = onCodeScanned
        let session = captureSession
        Task { @MainActor in
            session.stopRunning()
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            callback?(value)
        }
    }
}
#endif
