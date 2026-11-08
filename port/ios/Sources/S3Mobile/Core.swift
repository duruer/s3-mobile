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

    /// Documents/Settlers3: the user's Settlers III files (Files app, upload server).
    /// Documents/Data: settings, logs (log.txt of the last run).
    private init() {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let game = docs.appendingPathComponent("Settlers3", isDirectory: true)
        let data = docs.appendingPathComponent("Data", isDirectory: true)
        try? FileManager.default.createDirectory(at: game, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
        app = s3ios_create(game.path, data.path)
        audio.start(ring: s3ios_audio_handle(app))
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
