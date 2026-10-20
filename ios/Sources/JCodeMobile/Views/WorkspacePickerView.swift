import JCodeKit
import SwiftUI

struct WorkspacePickerView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @FocusState private var fieldFocused: Bool
    @State private var path = ""
    @State private var errorMessage: String?

    let isRequired: Bool

    var body: some View {
        NavigationStack {
            List {
                Section {
                    TextField("/Users/you/project", text: $path)
                        .font(Theme.mono(15))
                        .foregroundStyle(Theme.textPrimary)
                        .tint(Theme.mint)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .submitLabel(.go)
                        .focused($fieldFocused)
                        .onSubmit(open)
                        .listRowBackground(Theme.surface)
                        .accessibilityLabel("Workspace path")
                        .accessibilityHint("Absolute path of a folder on the server")
                        .accessibilityIdentifier("workspace-path")
                    Button(action: open) {
                        Label("Open workspace", systemImage: "folder")
                            .foregroundStyle(canOpen ? Theme.mint : Theme.textTertiary)
                    }
                    .disabled(!canOpen)
                    .accessibilityIdentifier("workspace-open")
                    .listRowBackground(Theme.surface)
                } header: {
                    Text("Folder on \(model.activeServer?.serverName ?? "server")")
                } footer: {
                    Text(footerError ?? "New sessions run tools and edit files in this folder on the server. Use an absolute path.")
                        .foregroundStyle(footerError == nil ? Theme.textTertiary : Theme.error)
                }

                if let recents = model.activeServer?.workspaces, !recents.isEmpty {
                    Section("Recent") {
                        ForEach(recents, id: \.self) { recent in
                            recentRow(recent)
                        }
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.background)
            .listStyle(.insetGrouped)
            .dynamicTypeSize(.large ... .accessibility3)
            .navigationTitle("Workspace")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if !isRequired {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { dismiss() }
                    }
                } else if let server = model.activeServer {
                    ToolbarItem(placement: .cancellationAction) {
                        Menu("Change server") {
                            ForEach(model.servers.filter { $0.id != server.id }) { other in
                                Button(other.serverName) { model.connect(to: other) }
                            }
                            Button("Pair new server", systemImage: "plus") { model.leaveServer() }
                        }
                        .accessibilityHint("Switches to another saved server or pairs a new one")
                        .accessibilityIdentifier("workspace-change-server")
                    }
                }
            }
        }
        .preferredColorScheme(.dark)
        .interactiveDismissDisabled(isRequired)
        .onAppear {
            path = suggestedPath
            fieldFocused = model.activeServer?.workspaces.isEmpty ?? true
        }
    }

    private func recentRow(_ recent: String) -> some View {
        let isActive = recent == model.activeWorkspace
        return Button {
            choose(recent)
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text(Workspace.displayName(recent))
                        .font(.body)
                        .foregroundStyle(Theme.textPrimary)
                        .lineLimit(1)
                    Text(recent)
                        .font(Theme.mono(11))
                        .foregroundStyle(Theme.textTertiary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
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
        .accessibilityLabel("Workspace \(Workspace.displayName(recent))")
        .accessibilityValue(isActive ? "Current" : "")
        .accessibilityHint("Starts a new session in this folder")
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
        .swipeActions {
            Button(role: .destructive) {
                model.forgetWorkspace(recent)
            } label: {
                Label("Forget", systemImage: "trash")
            }
        }
    }

    private var footerError: String? {
        errorMessage ?? (isRequired ? model.session.errorBanner : nil)
    }

    private var canOpen: Bool {
        Workspace.normalize(path) != nil
    }

    private var suggestedPath: String {
        guard let recent = model.activeWorkspace else { return "" }
        let parent = (recent as NSString).deletingLastPathComponent
        return parent == "/" ? "/" : parent + "/"
    }

    private func open() {
        guard canOpen else {
            errorMessage = "Enter an absolute path that starts with /"
            return
        }
        choose(path)
    }

    private func choose(_ candidate: String) {
        if model.selectWorkspace(candidate) {
            errorMessage = nil
            dismiss()
        } else {
            errorMessage = "Enter an absolute path that starts with /"
        }
    }
}
