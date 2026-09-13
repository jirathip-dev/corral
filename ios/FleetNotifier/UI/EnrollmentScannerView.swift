import AVFoundation
import SwiftUI
import UIKit

// MARK: - Enrollment QR scanner (#486)

/// Camera availability as the Add Host scanner presents it. A denied
/// camera permission (or a device with no camera at all — the Simulator)
/// is an actionable state with a way forward, never a dead screen.
enum EnrollmentCameraState: Equatable {
    /// Permission is undetermined or being requested.
    case checking
    /// A capture session is running.
    case ready
    /// Camera access is denied/restricted — offer the Settings route.
    case denied
    /// No camera can run here (no capture device, or the session could
    /// not be configured).
    case unavailable

    /// Pure mapping from the OS authorization value so the denial path is
    /// pinned by tests without a camera.
    static func forAuthorization(_ status: AVAuthorizationStatus) -> EnrollmentCameraState {
        switch status {
        case .authorized:
            return .ready
        case .denied, .restricted:
            return .denied
        case .notDetermined:
            return .checking
        @unknown default:
            return .unavailable
        }
    }
}

/// The Add Host QR scanner: a camera sheet that reads ONE host enrollment
/// code and hands the raw text to the model. It parses nothing and renders
/// nothing it scanned — the model owns the parse, the live-key
/// verification, and the transient redemption code, so pairing material
/// never enters a view (and cannot reach a screenshot, a log, or the
/// clipboard).
struct EnrollmentScannerView: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore
    @State private var cameraState: EnrollmentCameraState = .checking

    var body: some View {
        NavigationStack {
            VStack(spacing: 14) {
                switch cameraState {
                case .ready:
                    QRScannerPreview(onCode: { code in
                        Task { await model.handleScannedEnrollmentCode(code) }
                    }, onUnavailable: {
                        cameraState = .unavailable
                    })
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                    .accessibilityLabel("Camera preview. Point the camera at the host's enrollment QR code.")
                    guidance("Point the camera at the Corral host code shown on the host. The code grants read-only access only.")
                case .checking:
                    Spacer()
                    ProgressView()
                    Text("Waiting for camera access…")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                    Spacer()
                case .denied:
                    Spacer()
                    Label("Camera access is off for Corral.", systemImage: "video.slash")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(theme.text)
                    guidance("Allow the camera in iOS Settings to scan the host code, or pair by hand: close this and enter the host URL and registration token in the Add Host form.")
                    Button("Open iOS Settings") { openSettings() }
                        .accessibilityHint("Opens the Corral page in the system Settings app")
                    Spacer()
                case .unavailable:
                    Spacer()
                    Label("No camera is available here.", systemImage: "video.slash")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(theme.text)
                    guidance("Pair by hand instead: close this and enter the host URL and registration token in the Add Host form.")
                    Spacer()
                }
                if let errorMessage = model.addHostDraft.errorMessage {
                    // Scan-time failure (malformed/expired code, unreachable
                    // host, key mismatch, duplicate host): explicit and
                    // actionable; the scanner stays up for another attempt.
                    Label(errorMessage, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(theme.red)
                        .accessibilityLabel("Scan failed. \(errorMessage)")
                }
            }
            .padding()
            .navigationTitle("Scan host code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .task { await resolveCameraAccess() }
        .onChange(of: model.addHostDraft.enrollment) { _, enrollment in
            // A verified code moved the draft into the enrollment phase:
            // close the camera; the Add Host sheet shows identity + scope.
            if enrollment != nil {
                dismiss()
            }
        }
    }

    private func guidance(_ text: String) -> some View {
        Text(text)
            .font(.caption)
            .foregroundStyle(theme.subtext1)
            .multilineTextAlignment(.center)
    }

    /// Resolve the camera posture: no capture device (the Simulator) is
    /// `unavailable` first, then the OS authorization value decides,
    /// requesting access only while it is still undetermined.
    private func resolveCameraAccess() async {
        guard AVCaptureDevice.default(for: .video) != nil else {
            cameraState = .unavailable
            return
        }
        var state = EnrollmentCameraState.forAuthorization(
            AVCaptureDevice.authorizationStatus(for: .video))
        if state == .checking {
            let granted = await AVCaptureDevice.requestAccess(for: .video)
            state = granted ? .ready : .denied
        }
        cameraState = state
    }

    private func openSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        UIApplication.shared.open(url)
    }
}

/// The one-shot QR capture layer: reads the FIRST decoded QR string, stops
/// delivering, and tears the session down with the view. Frames are never
/// recorded, stored, or retained — only the decoded text is handed on.
private struct QRScannerPreview: UIViewRepresentable {
    let onCode: (String) -> Void
    let onUnavailable: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onCode: onCode, onUnavailable: onUnavailable)
    }

    func makeUIView(context: Context) -> ScannerPreviewView {
        let view = ScannerPreviewView()
        context.coordinator.configure(previewLayer: view.previewLayer)
        return view
    }

    func updateUIView(_ uiView: ScannerPreviewView, context: Context) {}

    static func dismantleUIView(_ uiView: ScannerPreviewView, coordinator: Coordinator) {
        coordinator.stop()
    }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate {
        private let session = AVCaptureSession()
        private let onCode: (String) -> Void
        private let onUnavailable: () -> Void
        private var delivered = false

        init(onCode: @escaping (String) -> Void, onUnavailable: @escaping () -> Void) {
            self.onCode = onCode
            self.onUnavailable = onUnavailable
        }

        func configure(previewLayer: AVCaptureVideoPreviewLayer) {
            session.beginConfiguration()
            defer { session.commitConfiguration() }
            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  session.canAddInput(input) else {
                onUnavailable()
                return
            }
            session.addInput(input)
            let output = AVCaptureMetadataOutput()
            guard session.canAddOutput(output) else {
                onUnavailable()
                return
            }
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            guard output.availableMetadataObjectTypes.contains(.qr) else {
                onUnavailable()
                return
            }
            output.metadataObjectTypes = [.qr]
            previewLayer.session = session
            previewLayer.videoGravity = .resizeAspectFill
            // startRunning blocks: keep it off the main thread.
            DispatchQueue.global(qos: .userInitiated).async { [session] in
                session.startRunning()
            }
        }

        func stop() {
            DispatchQueue.global(qos: .userInitiated).async { [session] in
                if session.isRunning {
                    session.stopRunning()
                }
            }
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput,
                            didOutput metadataObjects: [AVMetadataObject],
                            from connection: AVCaptureConnection) {
            guard !delivered,
                  let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
                  object.type == .qr,
                  let value = object.stringValue,
                  !value.isEmpty else { return }
            delivered = true
            onCode(value)
        }
    }
}

/// A UIView backed by the capture preview layer, laid out to the view.
private final class ScannerPreviewView: UIView {
    let previewLayer = AVCaptureVideoPreviewLayer()

    override init(frame: CGRect) {
        super.init(frame: frame)
        commonInit()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        commonInit()
    }

    private func commonInit() {
        previewLayer.videoGravity = .resizeAspectFill
        layer.addSublayer(previewLayer)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        previewLayer.frame = bounds
    }
}
