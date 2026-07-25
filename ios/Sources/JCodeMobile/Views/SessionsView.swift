import JCodeKit
import SwiftUI

/// Sessions surface: the primary navigation sheet, opened from the top-left of
/// the chat header.
///
/// This is where session switching lives (the frequent flow, so it is the first
/// thing in the list), plus servers, and a nested Settings row for the rare
/// configuration work. Settings is deliberately one level down: it is opened a
/// couple of times a month, while sessions are switched daily.
struct SessionsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var renameDraft = ""
    @State private var showRename = false
    @State private var showPairNew = false

    var body: some View {
        NavigationStack {
            List {
                sessionsSection
                SettingsServersSection(showPairNew: $showPairNew)
                moreSection
            }
            .scrollContentBackground(.hidden)
            .background(Theme.background)
            .dynamicTypeSize(.large ... .accessibility3)
            .navigationTitle("Sessions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .preferredColorScheme(.dark)
        .alert("Rename session", isPresented: $showRename) {
            TextField("Title", text: $renameDraft)
            Button("Rename") { model.renameSession(renameDraft) }
            Button("Cancel", role: .cancel) {}
        }
        .sheet(isPresented: $showPairNew) {
            NavigationStack {
                PairingView()
                    .background(Theme.background)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Cancel") { showPairNew = false }
                        }
                    }
            }
            .preferredColorScheme(.dark)
        }
        .onChange(of: model.activeServer?.id) { showPairNew = false }
    }

    /// Sessions first: switching is the most frequent reason to open this sheet.
    private var sessionsSection: some View {
        Section("Sessions") {
            Button {
                model.clearConversation()
                dismiss()
            } label: {
                Label("New session", systemImage: "plus")
                    .foregroundStyle(Theme.mint)
            }
            .listRowBackground(Theme.surface)
            .accessibilityHint("Clears the conversation and starts fresh")

            ForEach(model.session.allSessions, id: \.self) { sessionID in
                sessionRow(sessionID)
            }
        }
    }

    /// Rare configuration, one level down from the frequent flows.
    private var moreSection: some View {
        Section {
            Button {
                renameDraft = model.session.sessionTitle ?? ""
                showRename = true
            } label: {
                Label("Rename current session", systemImage: "pencil")
                    .foregroundStyle(Theme.textPrimary)
            }
            .listRowBackground(Theme.surface)
            .accessibilityHint("Opens a field to rename the active session")

            Button {
                model.compactConversation()
                dismiss()
            } label: {
                Label("Compact conversation", systemImage: "arrow.down.right.and.arrow.up.left")
                    .foregroundStyle(Theme.textPrimary)
            }
            .listRowBackground(Theme.surface)
            .accessibilityHint("Summarizes older messages to free context")

            NavigationLink {
                SettingsView()
            } label: {
                Label("Settings", systemImage: "gearshape")
                    .foregroundStyle(Theme.textPrimary)
            }
            .listRowBackground(Theme.surface)
            .accessibilityHint("Reasoning effort, server info, and diagnostics")
        }
    }

    private func sessionRow(_ sessionID: String) -> some View {
        let isActive = sessionID == model.session.sessionID
        let title = model.session.title(forSession: sessionID)
        return Button {
            model.switchSession(sessionID)
            dismiss()
        } label: {
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    if let title {
                        Text(title)
                            .font(.body)
                            .foregroundStyle(Theme.textPrimary)
                            .lineLimit(1)
                    }
                    Text(SessionsView.shortSessionID(sessionID))
                        .font(Theme.mono(title == nil ? 13 : 11))
                        .foregroundStyle(title == nil ? Theme.textPrimary : Theme.textTertiary)
                        .lineLimit(1)
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
        .accessibilityLabel("Session \(title ?? SessionsView.shortSessionID(sessionID))")
        .accessibilityValue(isActive ? "Current" : "")
        .accessibilityHint("Switches to this session")
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
    }

    static func shortSessionID(_ id: String) -> String {
        id.count > 24 ? String(id.prefix(24)) + "…" : id
    }
}
