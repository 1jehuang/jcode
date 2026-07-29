import JCodeKit
import SwiftUI

/// First-run pairing: scan QR or type host/port/code.
struct PairingView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var host = ""
    @State private var port = String(Gateway.defaultPort)
    @State private var code = ""
    @State private var isPairing = false
    @State private var errorMessage: String?
    @State private var showScanner = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                header

                if let errorMessage {
                    ErrorBanner(message: errorMessage) {
                        self.errorMessage = nil
                    }
                    .padding(.horizontal, -16)
                }

                Card {
                    VStack(alignment: .leading, spacing: 16) {
                        field("Host", text: $host, placeholder: "devbox.tailnet.ts.net")
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .keyboardType(.URL)
                        field("Port", text: $port, placeholder: "7643")
                            .keyboardType(.numberPad)
                        field("Pairing code", text: $code, placeholder: "123456")
                            .keyboardType(.numberPad)
                    }
                }

                Button(action: pair) {
                    HStack(spacing: 8) {
                        if isPairing && !reduceMotion {
                            ProgressView().tint(.black).controlSize(.small)
                        }
                        Text(isPairing ? "Pairing..." : "Pair")
                            .font(.headline)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 16)
                    .background {
                        if canPair {
                            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                                .fill(Theme.mintGradient)
                        } else {
                            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                                .fill(Theme.surfaceElevated)
                        }
                    }
                    .foregroundStyle(canPair ? Color.black : Theme.textTertiary)
                }
                .buttonStyle(PressableButtonStyle(scale: 0.98))
                .disabled(!canPair || isPairing)
                .animation(.easeOut(duration: 0.15), value: canPair)
                .accessibilityLabel("Pair")
                .accessibilityHint("Connects using the host, port, and code above")

                Button {
                    showScanner = true
                } label: {
                    Label("Scan QR from `jcode pair`", systemImage: "qrcode.viewfinder")
                        .font(.subheadline)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 14)
                        .background(Theme.surface)
                        .foregroundStyle(Theme.textPrimary)
                        .clipShape(
                            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                                .stroke(Theme.border, lineWidth: 1)
                        )
                }
                .buttonStyle(PressableButtonStyle(scale: 0.98))
                .accessibilityLabel("Scan QR code")
                .accessibilityHint("Opens the camera to scan a pairing code")

                Text("Run `jcode pair` on your machine, then scan the QR code or enter the code manually. Traffic stays on your tailnet.")
                    .font(.footnote)
                    .foregroundStyle(Theme.textTertiary)
            }
            .padding(16)
        }
        .scrollDismissesKeyboard(.interactively)
        .dynamicTypeSize(.large ... .accessibility3)
        .sheet(isPresented: $showScanner) {
            QRScannerView { scanned in
                showScanner = false
                if let payload = PairURI.parse(scanned) {
                    host = payload.gateway.host
                    port = String(payload.gateway.port)
                    code = payload.code
                    pair()
                } else {
                    errorMessage = "Not a jcode pairing QR code"
                }
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "terminal.fill")
                    .font(Theme.icon(20, weight: .semibold))
                    .foregroundStyle(Theme.mint)
                    .frame(width: 40, height: 40)
                    .background(Theme.surface)
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay(
                        RoundedRectangle(cornerRadius: 12, style: .continuous)
                            .stroke(Theme.border, lineWidth: 1)
                    )
                    .accessibilityHidden(true)
                Text("jcode")
                    .font(Theme.mono(32, weight: .bold))
                    .foregroundStyle(Theme.textPrimary)
            }
            Text("Pair with a server on your tailnet")
                .font(.subheadline)
                .foregroundStyle(Theme.textSecondary)
        }
        .padding(.top, 28)
        .padding(.bottom, 4)
    }

    private var canPair: Bool {
        !host.trimmingCharacters(in: .whitespaces).isEmpty && !code.isEmpty
            && UInt16(port) != nil
    }

    private func field(_ label: String, text: Binding<String>, placeholder: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.caption.weight(.medium))
                .foregroundStyle(Theme.textTertiary)
                .textCase(.uppercase)
                .tracking(0.5)
            TextField(placeholder, text: text)
                .font(Theme.mono(16))
                .foregroundStyle(Theme.textPrimary)
                .tint(Theme.mint)
                .padding(12)
                .background(Theme.surfaceElevated)
                .clipShape(
                    RoundedRectangle(cornerRadius: Theme.Radius.small, style: .continuous)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.Radius.small, style: .continuous)
                        .stroke(Theme.border, lineWidth: 1)
                )
                .accessibilityLabel(label)
        }
    }

    private func pair() {
        guard let portValue = UInt16(port) else { return }
        let gateway = Gateway(host: host.trimmingCharacters(in: .whitespaces), port: portValue)
        let pairCode = code
        isPairing = true
        errorMessage = nil
        Task {
            defer { isPairing = false }
            do {
                try await model.pair(
                    gateway: gateway,
                    code: pairCode,
                    deviceName: UIDevice.current.name
                )
            } catch let error as PairingClient.PairingError {
                switch error {
                case .invalidCode(let message):
                    errorMessage = message
                case .serverError(_, let message):
                    errorMessage = message
                case .invalidResponse:
                    errorMessage = "Unexpected response from server"
                }
            } catch {
                errorMessage = "Could not reach \(gateway.host):\(gateway.port)"
            }
        }
    }
}
