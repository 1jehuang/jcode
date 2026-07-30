import SwiftUI

@main
struct JCodeMobileApp: App {
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .preferredColorScheme(.dark)
        }
        .onChange(of: scenePhase) { _, phase in
            // iOS suspends sockets in the background; reconnect eagerly when
            // the user returns instead of waiting for a receive to fail and
            // back off. Connection.start resyncs history on resubscribe, so
            // this is safe even when the socket survived the suspension.
            if phase == .active, model.activeServer != nil, !model.isConnected {
                model.retryConnection()
            }
        }
    }
}
