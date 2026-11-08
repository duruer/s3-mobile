// swift-tools-version: 6.0
// Settlers 3 Mobile, iOS shell (docs/porting/ios.md). Built with xtool (`xtool dev`) after
// `port/scripts/build-ios.sh` has built the Rust core `libs3ios.a`.

import Foundation
import PackageDescription

/// Folder of this package (absolute), so the linker finds the Rust library wherever SwiftPM runs it.
let here = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let rustLib = "\(here)/../target/aarch64-apple-ios/release"

let package = Package(
    name: "S3Mobile",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        // An xtool project contains exactly one library product: the app.
        .library(name: "S3Mobile", targets: ["S3Mobile"]),
    ],
    targets: [
        // C API of the Rust core (include/s3ios.h; implemented in crates/s3-ios/src/lib.rs).
        .target(name: "CS3iOS"),
        .target(
            name: "S3Mobile",
            dependencies: ["CS3iOS"],
            linkerSettings: [
                .unsafeFlags(["-L\(rustLib)"]),
                .linkedLibrary("s3ios"),
                .linkedFramework("OpenGLES"),
                .linkedFramework("GLKit"),
                .linkedFramework("AVFoundation"),
                .linkedFramework("AudioToolbox"),
            ]
        ),
    ],
    // Swift 5 mode: the shell passes raw C pointers between the main and audio threads on purpose.
    swiftLanguageModes: [.v5]
)
