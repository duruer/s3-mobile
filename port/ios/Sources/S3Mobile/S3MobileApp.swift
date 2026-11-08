// Settlers 3 Mobile, iOS shell entry point (docs/porting/ios.md).
// The Rust core (libs3ios.a) does the launcher, the game and the drawing; this shell hosts it in UIKit.

import SwiftUI
import UIKit

@main
struct S3MobileApp: App {
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            GameViewRepresentable()
                .ignoresSafeArea()
                .statusBarHidden(true)
                .persistentSystemOverlays(.hidden)
                .defersSystemGestures(on: .all)
        }
        .onChange(of: phase) { _, p in
            Core.shared.setBackground(p != .active)
        }
    }
}

/// The SwiftUI wrapper of the game view controller.
struct GameViewRepresentable: UIViewControllerRepresentable {
    func makeUIViewController(context: Context) -> GameViewController { GameViewController() }
    func updateUIViewController(_ vc: GameViewController, context: Context) {}
}

final class GameViewController: UIViewController {
    override func loadView() {
        view = GameView(frame: UIScreen.main.bounds)
    }

    override var prefersStatusBarHidden: Bool { true }
    override var prefersHomeIndicatorAutoHidden: Bool { true }
    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge { .all }
    override var supportedInterfaceOrientations: UIInterfaceOrientationMask { .landscape }
}
