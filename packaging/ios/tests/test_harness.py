"""Portable build-phase contract checks; no Rust compilation or signing required."""
import importlib.util
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class HarnessTests(unittest.TestCase):
    def test_ipados_27_declares_a_single_window_scene(self):
        with (ROOT / 'VectorCraft/Info.plist').open('rb') as source:
            info = plistlib.load(source)
        manifest = info['UIApplicationSceneManifest']
        self.assertFalse(manifest['UIApplicationSupportsMultipleScenes'])
        configurations = manifest['UISceneConfigurations']['UIWindowSceneSessionRoleApplication']
        self.assertEqual(len(configurations), 1)
        self.assertEqual(configurations[0]['UISceneClassName'], 'UIWindowScene')
        self.assertEqual(configurations[0]['UISceneDelegateClassName'], 'VectorCraftSceneDelegate')
        self.assertNotIn('UISceneStoryboardFile', configurations[0])

    def test_rejects_resource_only_bundle(self):
        spec = importlib.util.spec_from_file_location('bundle_validation', ROOT / 'validate-bundle.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory)
            (bundle / 'Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'VectorCraft'}))
            with self.assertRaisesRegex(ValueError, 'Missing bundle executable'):
                module.validate(bundle)
            (bundle / 'VectorCraft').write_text('not a native executable')
            with self.assertRaisesRegex(ValueError, 'not Mach-O'):
                module.validate(bundle)

    def test_bundle_contract(self):
        with (ROOT / 'VectorCraft/Info.plist').open('rb') as source:
            info = plistlib.load(source)
        self.assertFalse(info['UIRequiresFullScreen'])
        self.assertEqual(len(info['CFBundleDocumentTypes']), 4)
        with (ROOT / 'VectorCraft/VectorCraft.entitlements').open('rb') as source:
            self.assertEqual(plistlib.load(source), {})

    def test_build_architectures_profiles_and_paths(self):
        for platform, arch, configuration, target, profile in [
            ('iphonesimulator', 'arm64', 'Debug', 'aarch64-apple-ios-sim', 'debug'),
            ('iphonesimulator', 'x86_64', 'Release', 'x86_64-apple-ios', 'release'),
            ('iphoneos', 'arm64', 'Release', 'aarch64-apple-ios', 'release'),
        ]:
            with self.subTest(target=target), tempfile.TemporaryDirectory(prefix='ios harness ') as directory:
                work = Path(directory)
                cargo = work / 'cargo mock'
                cargo.write_text('''#!/bin/sh
printf '%s\\n' "$@" > "$TEST_ARGUMENTS"
while [ "$#" -gt 0 ]; do
if [ "$1" = --target ]; then shift; target=$1; fi
if [ "$1" = --release ]; then profile=release; fi
shift
done
mkdir -p "$CARGO_TARGET_DIR/$target/${profile:-debug}"
touch "$CARGO_TARGET_DIR/$target/${profile:-debug}/libvectorcraft_ios.a"
''')
                cargo.chmod(0o755)
                xcrun = work / 'xcrun'
                xcrun.write_text('#!/bin/sh\nwhile [ "$#" -gt 0 ]; do if [ "$1" = -output ]; then shift; touch "$1"; exit; fi; shift; done\n')
                xcrun.chmod(0o755)
                args = work / 'arguments'
                env = dict(os.environ, SRCROOT=str(ROOT), CARGO=str(cargo),
                           CARGO_TARGET_DIR=str(work / 'rust target'), BUILT_PRODUCTS_DIR=str(work / 'products'),
                           PLATFORM_NAME=platform, ARCHS=arch, CONFIGURATION=configuration,
                           TEST_ARGUMENTS=str(args), PATH=str(work) + ':' + os.environ['PATH'])
                subprocess.run([str(ROOT / 'build-rust.sh')], env=env, check=True)
                arguments = args.read_text().splitlines()
                self.assertIn(target, arguments)
                self.assertIn('--lib', arguments)
                self.assertEqual('--release' in arguments, profile == 'release')
                self.assertTrue((work / 'products/rust/libvectorcraft_ios.a').exists())

    def test_link_phase_creates_executable(self):
        with tempfile.TemporaryDirectory(prefix='ios link ') as directory:
            work = Path(directory)
            tool = work / 'xcrun'
            tool.write_text("""#!/bin/sh
printf '%s\\n' "$*" >> "$TEST_ARGUMENTS"
if [ "$1" = otool ]; then printf 'MH_MAGIC_64 ARM64 EXECUTE\\n'; exit 0; fi
while [ "$#" -gt 0 ]; do
if [ "$1" = -o ] || [ "$1" = -output ]; then shift; touch "$1"; exit 0; fi
shift
done
exit 1
""")
            tool.chmod(0o755)
            env = dict(os.environ, PATH=str(work) + ':' + os.environ['PATH'],
                       TARGET_TEMP_DIR=str(work / 'temp'), TARGET_BUILD_DIR=str(work / 'products'),
                       WRAPPER_NAME='VectorCraft.app', EXECUTABLE_PATH='VectorCraft.app/VectorCraft',
                       PLATFORM_NAME='iphonesimulator', ARCHS='arm64 x86_64',
                       IPHONEOS_DEPLOYMENT_TARGET='17.0', SDKROOT='/test SDK',
                       BUILT_PRODUCTS_DIR=str(work / 'products'), TEST_ARGUMENTS=str(work / 'args'))
            subprocess.run([str(ROOT / 'link-rust.sh')], env=env, check=True)
            self.assertTrue((work / 'products/VectorCraft.app/VectorCraft').is_file())
            arguments = (work / 'args').read_text()
            self.assertIn('arm64-apple-ios17.0-simulator', arguments)
            self.assertIn('x86_64-apple-ios17.0-simulator', arguments)

    def test_rejects_unknown_platform(self):
        env = dict(os.environ, SRCROOT=str(ROOT), PLATFORM_NAME='macosx', CARGO='/usr/bin/true')
        result = subprocess.run([str(ROOT / 'build-rust.sh')], env=env, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'Unsupported Apple platform', result.stderr)


if __name__ == '__main__':
    unittest.main()
