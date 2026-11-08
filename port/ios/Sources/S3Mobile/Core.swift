// The Rust core handle (one per process): paths, lifecycle, audio.

import AVFoundation
import CS3iOS
import Foundation
import UIKit

final class Core {
    static let shared = Core()

    let app: OpaquePointer
    let audio = AudioOut()
    private var background = false
    /// The app's own game folder.
    let appGameDir: URL
    /// A folder the user picked in the Files picker (outside the app, kept when the app is deleted), with its
    /// security-scoped access running.
    private var picked: URL?
    private static let bookmarkKey = "gameFolderBookmark"

    /// Documents/Settlers3: the user's Settlers III files (Files app, upload server), unless a folder was picked.
    /// Documents/Data: settings, logs (log.txt of the last run).
    private init() {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let game = docs.appendingPathComponent("Settlers3", isDirectory: true)
        let data = docs.appendingPathComponent("Data", isDirectory: true)
        try? FileManager.default.createDirectory(at: game, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
        appGameDir = game
        app = s3ios_create(game.path, data.path)
        audio.start(ring: s3ios_audio_handle(app))
        restorePickedFolder()
    }

    /// Re-open the folder picked in an earlier run (its bookmark).
    private func restorePickedFolder() {
        guard let data = UserDefaults.standard.data(forKey: Self.bookmarkKey) else { return }
        var stale = false
        guard let url = try? URL(resolvingBookmarkData: data, options: [], relativeTo: nil, bookmarkDataIsStale: &stale),
              url.startAccessingSecurityScopedResource() else {
            NSLog("s3: picked game folder no longer accessible; using the app folder")
            return
        }
        if stale, let fresh = try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil) {
            UserDefaults.standard.set(fresh, forKey: Self.bookmarkKey)
        }
        picked = url
        s3ios_set_game_dir(app, url.path, 1)
    }

    /// The user picked `url` in the folder picker: keep access (bookmark) and use it as the game folder.
    func usePickedFolder(_ url: URL) {
        guard url.startAccessingSecurityScopedResource() else {
            NSLog("s3: no access to the picked folder \(url.path)")
            return
        }
        picked?.stopAccessingSecurityScopedResource()
        picked = url
        if let data = try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil) {
            UserDefaults.standard.set(data, forKey: Self.bookmarkKey)
        }
        s3ios_set_game_dir(app, url.path, 1)
    }

    /// Back to the app's own game folder.
    func useAppFolder() {
        picked?.stopAccessingSecurityScopedResource()
        picked = nil
        UserDefaults.standard.removeObject(forKey: Self.bookmarkKey)
        s3ios_set_game_dir(app, appGameDir.path, 0)
    }

    func setBackground(_ bg: Bool) {
        guard bg != background else { return }
        background = bg
        s3ios_set_background(app, bg ? 1 : 0)
        if bg {
            audio.pause()
        } else {
            audio.resume()
        }
    }
}

/// Game sound: an AVAudioSourceNode pulling interleaved stereo int16 at 44100 Hz from the Rust ring and converting it
/// to the engine's float format.
final class AudioOut {
    private let engine = AVAudioEngine()
    private var node: AVAudioSourceNode?
    private var ring: OpaquePointer?
    private let scratch = UnsafeMutablePointer<Int16>.allocate(capacity: 16384 * 2)

    func start(ring: OpaquePointer?) {
        self.ring = ring
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.soloAmbient)
        try? session.setActive(true)
        guard let fmt = AVAudioFormat(standardFormatWithSampleRate: 44100, channels: 2) else { return }
        let scratch = self.scratch
        let n = AVAudioSourceNode(format: fmt) { _, _, frameCount, abl -> OSStatus in
            let frames = Int(min(frameCount, 16384))
            if let r = ring {
                s3ios_audio_pull(r, scratch, UInt32(frames))
            } else {
                scratch.update(repeating: 0, count: frames * 2)
            }
            let bufs = UnsafeMutableAudioBufferListPointer(abl)
            for (ch, buf) in bufs.enumerated() {
                guard let out = buf.mData?.assumingMemoryBound(to: Float.self) else { continue }
                if bufs.count == 1 {
                    // Interleaved (not expected with the standard format).
                    for i in 0..<frames * 2 { out[i] = Float(scratch[i]) / 32768 }
                } else {
                    for i in 0..<frames { out[i] = Float(scratch[i * 2 + min(ch, 1)]) / 32768 }
                }
            }
            return noErr
        }
        node = n
        engine.attach(n)
        engine.connect(n, to: engine.mainMixerNode, format: fmt)
        do {
            try engine.start()
        } catch {
            NSLog("s3: audio engine start failed: \(error)")
        }
    }

    func pause() { engine.pause() }

    func resume() {
        try? AVAudioSession.sharedInstance().setActive(true)
        try? engine.start()
    }
}
