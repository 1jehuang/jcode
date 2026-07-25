import JCodeKit
import SwiftUI

/// Model picker, opened by tapping the model name in the chat header.
///
/// Changing model is a frequent, in-conversation decision (the mined usage
/// profile puts it well above pairing or renaming), so it gets its own
/// one-tap-from-chat surface instead of living inside Settings.
struct ModelPickerView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                if model.session.availableModels.isEmpty {
                    Section {
                        Text("No models reported by the server yet.")
                            .font(.callout)
                            .foregroundStyle(Theme.textSecondary)
                            .listRowBackground(Theme.surface)
                    }
                } else {
                    Section("Model") {
                        ForEach(model.session.availableModels, id: \.self) { name in
                            modelRow(name)
                        }
                    }
                }

                if model.session.reasoningEffort != nil {
                    Section("Reasoning effort") {
                        ForEach(SettingsView.reasoningEfforts, id: \.self) { effort in
                            effortRow(effort)
                        }
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.background)
            .dynamicTypeSize(.large ... .accessibility3)
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .preferredColorScheme(.dark)
    }

    private func modelRow(_ name: String) -> some View {
        let isActive = name == model.session.modelName
        return Button {
            model.setModel(name)
            // Picking a model is the whole point of this sheet, so close it and
            // return the user to the conversation immediately.
            dismiss()
        } label: {
            HStack {
                Text(name)
                    .font(Theme.mono(13))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1)
                Spacer()
                if isActive {
                    Image(systemName: "checkmark")
                        .font(.caption)
                        .foregroundStyle(Theme.mint)
                        .accessibilityHidden(true)
                }
            }
        }
        .listRowBackground(Theme.surface)
        .accessibilityLabel("Model \(name)")
        .accessibilityValue(isActive ? "Selected" : "")
        .accessibilityHint("Switches the session to this model")
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
    }

    private func effortRow(_ effort: String) -> some View {
        let isActive = effort == model.session.reasoningEffort
        return Button {
            model.setReasoningEffort(effort)
        } label: {
            HStack {
                Text(effort)
                    .font(Theme.mono(13))
                    .foregroundStyle(Theme.textPrimary)
                Spacer()
                if isActive {
                    Image(systemName: "checkmark")
                        .font(.caption)
                        .foregroundStyle(Theme.mint)
                        .accessibilityHidden(true)
                }
            }
        }
        .listRowBackground(Theme.surface)
        .accessibilityLabel("Reasoning effort \(effort)")
        .accessibilityValue(isActive ? "Selected" : "")
        .accessibilityHint("Sets how much the model reasons before answering")
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
    }
}
