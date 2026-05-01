import Foundation
@preconcurrency import Speech
@preconcurrency import AVFoundation

@MainActor
final class SpeechRecognizer: ObservableObject {
    enum State: Equatable {
        case idle
        case requesting
        case recording
        case error(String)
    }

    @Published var state: State = .idle
    @Published var transcript: String = ""

    private var recognizer: SFSpeechRecognizer?
    private let audioCapture = SpeechAudioCapture()

    init() {
        recognizer = SFSpeechRecognizer(locale: Locale(identifier: "en-US"))
    }

    deinit {
        audioCapture.stop(cancelRecognition: true)
    }

    var isRecording: Bool { state == .recording }

    func toggleRecording() {
        if isRecording {
            stopRecording()
        } else {
            Task { await startRecording() }
        }
    }

    func startRecording() async {
        guard state != .recording, state != .requesting else { return }

        state = .requesting
        transcript = ""

        let speechStatus = await withCheckedContinuation { (cont: CheckedContinuation<SFSpeechRecognizerAuthorizationStatus, Never>) in
            SFSpeechRecognizer.requestAuthorization { status in
                Task { @MainActor in
                    cont.resume(returning: status)
                }
            }
        }

        guard speechStatus == .authorized else {
            state = .error("Speech recognition not authorized")
            return
        }

        guard let recognizer = recognizer, recognizer.isAvailable else {
            state = .error("Speech recognizer unavailable")
            return
        }

        audioCapture.start(
            recognizer: recognizer,
            onStarted: { [weak self] in
                Task { @MainActor in
                    guard let self else { return }
                    self.state = .recording
                }
            },
            onTranscript: { [weak self] text in
                Task { @MainActor in
                    self?.transcript = text
                }
            },
            onFinished: { [weak self] in
                Task { @MainActor in
                    guard let self else { return }
                    if self.state == .recording || self.state == .requesting {
                        self.state = .idle
                    }
                }
            },
            onError: { [weak self] message in
                Task { @MainActor in
                    self?.state = .error(message)
                }
            }
        )
    }

    func stopRecording() {
        guard state == .recording || state == .requesting else { return }
        audioCapture.stop(cancelRecognition: true)
        state = .idle
    }
}

private final class SpeechAudioCapture: @unchecked Sendable {
    private let queue = DispatchQueue(label: "com.jcode.mobile.speech.audio")
    private var engine: AVAudioEngine?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var tapInstalled = false

    func start(
        recognizer: SFSpeechRecognizer,
        onStarted: @escaping @Sendable () -> Void,
        onTranscript: @escaping @Sendable (String) -> Void,
        onFinished: @escaping @Sendable () -> Void,
        onError: @escaping @Sendable (String) -> Void
    ) {
        queue.async { [weak self] in
            guard let self else { return }

            self.stopLocked(cancelRecognition: true, deactivateSession: true)

            let audioSession = AVAudioSession.sharedInstance()
            do {
                try audioSession.setCategory(.record, mode: .measurement, options: .duckOthers)
                try audioSession.setActive(true, options: .notifyOthersOnDeactivation)
            } catch {
                onError("Audio session failed")
                return
            }

            let engine = AVAudioEngine()
            let request = SFSpeechAudioBufferRecognitionRequest()
            request.shouldReportPartialResults = true
            request.addsPunctuation = true

            let inputNode = engine.inputNode
            let recordingFormat = inputNode.outputFormat(forBus: 0)
            inputNode.installTap(onBus: 0, bufferSize: 1024, format: recordingFormat) { [weak request] buffer, _ in
                request?.append(buffer)
            }

            self.engine = engine
            self.request = request
            self.tapInstalled = true

            self.task = recognizer.recognitionTask(with: request) { [weak self] result, error in
                if let result {
                    onTranscript(result.bestTranscription.formattedString)
                }

                if error != nil || (result?.isFinal ?? false) {
                    guard let capture = self else { return }
                    capture.queue.async {
                        capture.stopLocked(cancelRecognition: false, deactivateSession: true)
                        onFinished()
                    }
                }
            }

            do {
                engine.prepare()
                try engine.start()
                onStarted()
            } catch {
                self.stopLocked(cancelRecognition: true, deactivateSession: true)
                onError("Could not start audio engine")
            }
        }
    }

    func stop(cancelRecognition: Bool) {
        queue.async { [weak self] in
            self?.stopLocked(cancelRecognition: cancelRecognition, deactivateSession: true)
        }
    }

    private func stopLocked(cancelRecognition: Bool, deactivateSession: Bool) {
        if let engine {
            if tapInstalled {
                engine.inputNode.removeTap(onBus: 0)
                tapInstalled = false
            }

            if engine.isRunning {
                engine.stop()
            }
        }

        request?.endAudio()

        if cancelRecognition {
            task?.cancel()
        }

        task = nil
        request = nil
        engine = nil

        if deactivateSession {
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        }
    }
}
