// Full-screen GL view: draws through the Rust core (GLES via OpenGLES.framework), forwards touches, the soft
// keyboard (UIKeyInput) and hardware keys.

import CS3iOS
import GLKit
import UIKit
import UniformTypeIdentifiers

final class GameView: GLKView, UIKeyInput, UIDocumentPickerDelegate {
    private var link: CADisplayLink?
    /// Fingers down, in touch order: (touch, id).
    private var fingers: [(UITouch, Int32)] = []
    private var nextId: Int32 = 1
    private let haptic = UIImpactFeedbackGenerator(style: .medium)
    private var lastSize: (UInt32, UInt32, UInt32) = (0, 0, 0)

    override init(frame: CGRect) {
        let ctx = EAGLContext(api: .openGLES3) ?? EAGLContext(api: .openGLES2)!
        super.init(frame: frame, context: ctx)
        drawableColorFormat = .RGBA8888
        drawableDepthFormat = .formatNone
        drawableStencilFormat = .formatNone
        enableSetNeedsDisplay = false
        isMultipleTouchEnabled = true
        contentScaleFactor = UIScreen.main.nativeScale
        _ = Core.shared
        let l = CADisplayLink(target: self, selector: #selector(step))
        l.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 120, preferred: 120)
        l.add(to: .main, forMode: .common)
        link = l
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    private var pixelSize: (UInt32, UInt32) {
        let s = contentScaleFactor
        return (UInt32(max(1, (bounds.width * s).rounded())), UInt32(max(1, (bounds.height * s).rounded())))
    }

    /// Approximate density: 163 dpi per point scale on iPhones (132 on iPads).
    private var dpi: UInt32 {
        let perPoint: CGFloat = UIDevice.current.userInterfaceIdiom == .pad ? 132 : 163
        return UInt32(perPoint * UIScreen.main.scale)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let (w, h) = pixelSize
        if (w, h, dpi) != lastSize {
            lastSize = (w, h, dpi)
            s3ios_resize(Core.shared.app, w, h, dpi)
            s3ios_set_refresh(Core.shared.app, UInt32(window?.screen.maximumFramesPerSecond ?? UIScreen.main.maximumFramesPerSecond))
        }
    }

    @objc private func step() {
        let out = s3ios_tick(Core.shared.app)
        if out.redraw != 0 {
            display()
        }
        if out.haptic_ms > 0 {
            haptic.impactOccurred(intensity: min(1.0, CGFloat(out.haptic_ms) / 40.0))
        }
        switch out.keyboard {
        case 1: _ = becomeFirstResponder()
        case 2: _ = resignFirstResponder()
        case 3: _ = isFirstResponder ? resignFirstResponder() : becomeFirstResponder()
        default: break
        }
        UIApplication.shared.isIdleTimerDisabled = out.in_game != 0
        switch out.pick_folder {
        case 1: showFolderPicker()
        case 2: Core.shared.useAppFolder()
        default: break
        }
        if out.quit != 0 {
            exit(0)
        }
    }

    // MARK: game folder picker

    private func showFolderPicker() {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder])
        picker.delegate = self
        picker.allowsMultipleSelection = false
        var vc = window?.rootViewController
        while let p = vc?.presentedViewController { vc = p }
        vc?.present(picker, animated: true)
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        if let url = urls.first {
            Core.shared.usePickedFolder(url)
        }
    }

    override func draw(_ rect: CGRect) {
        s3ios_draw(Core.shared.app, UInt32(drawableWidth), UInt32(drawableHeight))
    }

    // MARK: touches

    private func point(_ t: UITouch) -> (Float, Float) {
        let p = t.location(in: self)
        return (Float(p.x * contentScaleFactor), Float(p.y * contentScaleFactor))
    }

    private func send(_ action: Int32, _ t: UITouch, _ id: Int32, extra: (UITouch, Int32)? = nil) {
        var all = fingers
        if let e = extra { all.append(e) }
        var ids = all.map { $0.1 }
        var xs = all.map { point($0.0).0 }
        var ys = all.map { point($0.0).1 }
        let (x, y) = point(t)
        s3ios_touch(Core.shared.app, action, id, x, y, Int32(all.count), &ids, &xs, &ys)
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        for t in touches {
            let id = nextId
            nextId &+= 1
            let first = fingers.isEmpty
            fingers.append((t, id))
            send(first ? 0 : 1, t, id)
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let t = touches.first, let f = fingers.first(where: { $0.0 === t }) else { return }
        send(2, t, f.1)
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        for t in touches {
            guard let i = fingers.firstIndex(where: { $0.0 === t }) else { continue }
            let f = fingers.remove(at: i)
            send(fingers.isEmpty ? 4 : 3, t, f.1, extra: f)
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let f = fingers.first else { return }
        send(5, f.0, f.1)
        fingers.removeAll()
    }

    // MARK: soft keyboard (UIKeyInput)

    override var canBecomeFirstResponder: Bool { true }
    var hasText: Bool { true }
    var keyboardType: UIKeyboardType = .asciiCapable
    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no

    func insertText(_ text: String) {
        for u in text.unicodeScalars {
            s3ios_text(Core.shared.app, u.value)
        }
    }

    func deleteBackward() {
        s3ios_text(Core.shared.app, 8)
    }

    // MARK: hardware keyboard

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !forward(presses, down: true) { super.pressesBegan(presses, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !forward(presses, down: false) { super.pressesEnded(presses, with: event) }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if !forward(presses, down: false) { super.pressesCancelled(presses, with: event) }
    }

    /// Hardware keys -> Windows virtual keys + the characters they type. Handled presses do not reach UIKeyInput,
    /// so nothing arrives twice.
    private func forward(_ presses: Set<UIPress>, down: Bool) -> Bool {
        var used = false
        for p in presses {
            guard let key = p.key, let vk = Self.vk(key.keyCode) else { continue }
            s3ios_key(Core.shared.app, vk, down ? 1 : 0)
            if down, key.modifierFlags.intersection([.command, .control]).isEmpty {
                if vk == 0x08 || vk == 0x0d {
                    s3ios_char(Core.shared.app, vk == 0x08 ? 8 : 13)
                } else {
                    for u in key.characters.unicodeScalars where u.value >= 0x20 && u.value < 0xF700 {
                        s3ios_char(Core.shared.app, u.value)
                    }
                }
            }
            used = true
        }
        return used
    }

    private static func vk(_ k: UIKeyboardHIDUsage) -> UInt32? {
        let r = k.rawValue
        switch r {
        case 0x04...0x1d: return UInt32(0x41 + (r - 0x04)) // A..Z
        case 0x1e...0x26: return UInt32(0x31 + (r - 0x1e)) // 1..9
        case 0x27: return 0x30 // 0
        case 0x28, 0x58: return 0x0d // return, keypad enter
        case 0x29: return 0x1b // escape
        case 0x2a: return 0x08 // backspace
        case 0x2b: return 0x09 // tab
        case 0x2c: return 0x20 // space
        case 0x3a...0x45: return UInt32(0x70 + (r - 0x3a)) // F1..F12
        case 0x49: return 0x2d // insert
        case 0x4a: return 0x24 // home
        case 0x4b: return 0x21 // page up
        case 0x4c: return 0x2e // delete
        case 0x4d: return 0x23 // end
        case 0x4e: return 0x22 // page down
        case 0x4f: return 0x27 // right
        case 0x50: return 0x25 // left
        case 0x51: return 0x28 // down
        case 0x52: return 0x26 // up
        case 0xe0, 0xe4: return 0x11 // control
        case 0xe1, 0xe5: return 0x10 // shift
        case 0xe2, 0xe6: return 0x12 // alt / option
        default: return nil
        }
    }
}
