import SwiftUI
import PhotosUI

#if canImport(UIKit)
import UIKit

struct ImageAttachment: Identifiable, Equatable {
    let id = UUID()
    let image: UIImage
    let mediaType: String
    let base64Data: String

    static func == (lhs: ImageAttachment, rhs: ImageAttachment) -> Bool {
        lhs.id == rhs.id
    }

    static func from(image: UIImage, maxDimension: CGFloat = 1568) -> ImageAttachment? {
        let resized = image.resizedToFit(maxDimension: maxDimension)
        guard let data = resized.jpegData(compressionQuality: 0.85) else {
            return nil
        }

        let sizeLimit = 20 * 1024 * 1024
        guard data.count < sizeLimit else {
            return nil
        }

        return ImageAttachment(
            image: resized,
            mediaType: "image/jpeg",
            base64Data: data.base64EncodedString()
        )
    }
}

extension UIImage {
    func resizedToFit(maxDimension: CGFloat) -> UIImage {
        let maxSide = max(size.width, size.height)
        guard maxSide > maxDimension else { return self }
        let scale = maxDimension / maxSide
        let newSize = CGSize(width: size.width * scale, height: size.height * scale)
        let renderer = UIGraphicsImageRenderer(size: newSize)
        return renderer.image { _ in
            draw(in: CGRect(origin: .zero, size: newSize))
        }
    }
}

struct PhotoPickerButton: View {
    @Binding var attachments: [ImageAttachment]
    @State private var selectedItems: [PhotosPickerItem] = []

    var body: some View {
        PhotosPicker(selection: $selectedItems, maxSelectionCount: 4, matching: .images) {
            Image(systemName: "photo.on.rectangle")
                .font(.title3)
        }
        .onChange(of: selectedItems) {
            Task {
                for item in selectedItems {
                    guard let data = try? await item.loadTransferable(type: Data.self),
                          let uiImage = UIImage(data: data),
                          let attachment = ImageAttachment.from(image: uiImage) else {
                        continue
                    }
                    attachments.append(attachment)
                }
                selectedItems = []
            }
        }
    }
}

struct CameraButton: View {
    @Binding var attachments: [ImageAttachment]
    @State private var showCamera = false

    var body: some View {
        Button {
            showCamera = true
        } label: {
            Image(systemName: "camera")
                .font(.title3)
        }
        .fullScreenCover(isPresented: $showCamera) {
            CameraPickerView { image in
                if let attachment = ImageAttachment.from(image: image) {
                    attachments.append(attachment)
                }
            }
        }
    }
}

struct AttachmentStrip: View {
    @Binding var attachments: [ImageAttachment]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(attachments) { attachment in
                    ZStack(alignment: .topTrailing) {
                        Image(uiImage: attachment.image)
                            .resizable()
                            .aspectRatio(contentMode: .fill)
                            .frame(width: 60, height: 60)
                            .clipShape(RoundedRectangle(cornerRadius: 8))

                        Button {
                            attachments.removeAll { $0.id == attachment.id }
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(.caption)
                                .foregroundStyle(.white)
                                .background(Circle().fill(.black.opacity(0.6)))
                        }
                        .offset(x: 4, y: -4)
                    }
                }
            }
            .padding(.horizontal, 4)
        }
        .frame(height: 68)
    }
}

struct CameraPickerView: UIViewControllerRepresentable {
    let onImageCaptured: (UIImage) -> Void
    @Environment(\.dismiss) private var dismiss

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ uiViewController: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(onImageCaptured: onImageCaptured, dismiss: dismiss)
    }

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let onImageCaptured: (UIImage) -> Void
        let dismiss: DismissAction

        init(onImageCaptured: @escaping (UIImage) -> Void, dismiss: DismissAction) {
            self.onImageCaptured = onImageCaptured
            self.dismiss = dismiss
        }

        func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
            if let image = info[.originalImage] as? UIImage {
                onImageCaptured(image)
            }
            dismiss()
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
            dismiss()
        }
    }
}
#endif
