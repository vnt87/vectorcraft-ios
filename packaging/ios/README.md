# VectorCraft iPad bundle

This Xcode target packages the Rust `vectorcraft-ios` static library. It contains no
Swift or Objective-C product source. Open `VectorCraft.xcodeproj` and use the shared
`VectorCraft` scheme. Set your development team in Xcode for physical-device signing.

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
xcodebuild -project packaging/ios/VectorCraft.xcodeproj -scheme VectorCraft \
  -configuration Debug -destination 'generic/platform=iOS Simulator' build
python3 -m unittest discover -s packaging/ios/tests -v
python3 packaging/ios/validate-bundle.py path/to/VectorCraft.app
cargo xtask ios --sim
```

`xtask ios --sim` builds, boots, installs, and launches through the simulator CLI.
For a physical iPad pass `--device --device-id <udid> --team <id>`; `--bundle-id <id>` overrides
the bundle identifier for both build and launch when the product id cannot be registered to your
team. It does not require a desktop viewer. Xcode 27 provides Device Hub;
`--show-simulator` currently targets the older Simulator application.

The build phase resolves Cargo from `CARGO` or `$HOME/.cargo/bin` and builds only
`--lib`. It maps arm64 device, arm64 Simulator, and Intel Simulator architectures
and creates a universal archive when Xcode requests multiple architectures.
Debug uses Cargo's debug profile; Release uses `--release`. Artifacts default to
`target/ios-xcode`; `CARGO_TARGET_DIR` can override that location. Paths containing
spaces are supported. Rust targets must be installed before building.

The library must export Apple's ordinary C entry symbol `main`:
`extern "C" fn main(argc: i32, argv: *const *const c_char) -> i32`.
A dedicated link phase invokes Apple’s clang driver because Xcode skips its
standard link phase for targets with no C/Swift/Objective-C sources. It links each
requested architecture using Xcode’s SDK and deployment target, then creates the
bundle executable with lipo before signing. The linker pulls the Rust entry symbol
from the archive with `-u _main`. Release builds generate a dSYM.
The bundle validator checks that CFBundleExecutable exists, contains a Mach-O
EXECUTE header, and passes strict codesign verification. Use `--unsigned` only for
intentional unsigned builds. The runner owns
UIKit startup and must keep interop within its Apple bridge modules.

The plist permits iPad multitasking and declares opening documents in place.
The scene manifest declares one VectorCraft window scene, required by iPadOS 27.
Multiple scenes remain disabled until independent window ownership is implemented.
Document type declarations advertise supported formats; native document pickers
are implemented, with provider selection and write-back acceptance still pending.
The entitlement file is deliberately empty: iOS already sandboxes applications;
macOS app-sandbox and user-selected-file entitlements do not belong here.

The asset catalog's `AppIcon` is the VectorCraft kitsune from `assets/app-icon/`
(attributed in `ATTRIBUTION.md`), flattened onto the full-bleed `#2f7bf5` field with no
alpha channel, since iOS applies its own corner mask and rejects transparency. It is one
1024 pt image that Xcode scales to every iPad size.
