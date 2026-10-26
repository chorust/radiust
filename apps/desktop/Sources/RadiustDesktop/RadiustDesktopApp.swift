import SwiftUI

/// Radiust macOS Desktop entry.
///
/// Process model (locked): in-process Engine only. Quitting this app ends the
/// session — there is no daemon, launchd agent, or menubar resident process.
@main
struct RadiustDesktopApp: App {
    @StateObject private var sessionStore = DesktopSessionStore()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(sessionStore)
                .onAppear {
                    sessionStore.openIfNeeded()
                }
                .onDisappear {
                    // Window teardown; full quit also deinit's the store.
                }
        }
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button("About Radiust") {}
            }
        }

        Settings {
            SettingsView()
                .environmentObject(sessionStore)
        }
    }
}
