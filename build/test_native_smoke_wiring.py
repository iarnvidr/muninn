"""Regression coverage for the native macOS workflow's smoke bundle path."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent


class NativeSmokeWiringTests(unittest.TestCase):
    def test_workflow_smokes_the_signed_bundle_that_build_script_stages(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        builder = (ROOT / "build/build-native-macos.sh").read_text()
        smoke = (ROOT / "build/smoke-native-macos.sh").read_text()

        build_step = re.search(r'run: bash build/build-native-macos\.sh "\$RUNNER_TEMP/muninn"', workflow)
        smoke_step = re.search(r'run: bash build/smoke-native-macos\.sh "([^"]+)"', workflow)
        self.assertIsNotNone(build_step, "workflow must build into its runner-temp output directory")
        self.assertIsNotNone(smoke_step, "workflow must run native smoke")
        self.assertEqual(smoke_step.group(1), "$RUNNER_TEMP/muninn/Muninn.app")
        self.assertIn('ditto "$app" "$out/Muninn.app"', builder,
                      "builder must stage its exact signed .app at the workflow smoke path")
        self.assertLess(builder.index('codesign --verify --deep --strict --verbose=2 "$app"'),
                        builder.index('ditto "$app" "$out/Muninn.app"'))
        self.assertIn('[[ -d "$app/Contents/MacOS" ]]', smoke)
        self.assertIn('codesign --verify --deep --strict "$app"', smoke)


if __name__ == "__main__":
    unittest.main()
