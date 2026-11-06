from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


LAUNCHER = Path(__file__).resolve().parents[1] / "compat/linux-focal/start-nyaterm.sh"
GRAPHICS_ENV = (
    "LD_LIBRARY_PATH", "__EGL_VENDOR_LIBRARY_FILENAMES", "LIBGL_DRIVERS_PATH",
    "GALLIUM_DRIVER", "LIBGL_ALWAYS_SOFTWARE", "VK_DRIVER_FILES", "VK_ICD_FILENAMES",
    "__NV_PRIME_RENDER_OFFLOAD",
)


@unittest.skipUnless(os.name == "posix" and shutil.which("bash"), "POSIX bash launcher required")
class FocalLauncherTests(unittest.TestCase):
    def launch(self, args=(), settings=None, runtime=True):
        with tempfile.TemporaryDirectory(prefix="nyaterm renderer ") as directory:
            app = Path(directory)
            shutil.copy2(LAUNCHER, app / LAUNCHER.name)
            (app / LAUNCHER.name).chmod(0o755)
            probe = app / "nyaterm"
            probe.write_text("#!" + sys.executable + "\n" +
                             "import json, os, sys\n" +
                             "keys = " + repr((*GRAPHICS_ENV, "NYATERM_RENDERER")) + "\n" +
                             "print(json.dumps({'args': sys.argv[1:], 'env': {k: os.environ.get(k) for k in keys}}))\n" +
                             "sys.exit(int(os.environ.get('PROBE_EXIT', '0')))\n")
            probe.chmod(0o755)
            if runtime:
                for relative in ("lib/libEGL.so.1", "lib/libEGL_nyaterm.so.1",
                                 "share/glvnd/egl_vendor.d/50_mesa.json"):
                    path = app / "opengl" / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(b"fixture")
            environment = {k: v for k, v in os.environ.items()
                           if k not in (*GRAPHICS_ENV, "NYATERM_RENDERER", "PROBE_EXIT")}
            environment.update(settings or {})
            result = subprocess.run([str(app / LAUNCHER.name), *args], env=environment,
                                    text=True, capture_output=True)
            output = json.loads(result.stdout) if result.stdout else None
            return result, output, app

    def test_default_and_empty_setting_use_system_gpu_without_private_runtime(self):
        for settings in ({}, {"NYATERM_RENDERER": ""}):
            with self.subTest(settings=settings):
                result, output, _ = self.launch(["a file with spaces", "ssh://test"], settings, runtime=False)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(output["args"], ["a file with spaces", "ssh://test"])
                self.assertEqual(output["env"]["NYATERM_RENDERER"], "gpu")
                self.assertTrue(all(output["env"][key] is None for key in GRAPHICS_ENV))

    def test_gpu_preserves_explicit_system_driver_settings(self):
        settings = {key: "user-setting-" + key for key in GRAPHICS_ENV if key != "LD_LIBRARY_PATH"}
        settings["LD_LIBRARY_PATH"] = "/opt/custom/lib"
        settings["NYATERM_RENDERER"] = "gpu"
        result, output, _ = self.launch(settings=settings, runtime=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output["env"], settings)

    def test_software_flag_and_environment_use_bundled_runtime(self):
        for args, settings in ((["--software", "ssh://test"], {}),
                               (["ssh://test"], {"NYATERM_RENDERER": "software"})):
            for existing in (None, "", "/opt/custom/lib"):
                with self.subTest(args=args, existing=existing):
                    environment = dict(settings, LIBGL_ALWAYS_SOFTWARE="0", GALLIUM_DRIVER="iris",
                                       VK_DRIVER_FILES="/system/gpu.json", VK_ICD_FILENAMES="/system/gpu.json")
                    if existing is not None:
                        environment["LD_LIBRARY_PATH"] = existing
                    result, output, app = self.launch(args, environment)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(output["args"], ["ssh://test"])
                    selected = output["env"]
                    self.assertEqual(selected["NYATERM_RENDERER"], "software")
                    self.assertEqual(selected["LD_LIBRARY_PATH"], str(app / "opengl/lib") +
                                     (":" + existing if existing else ""))
                    self.assertEqual(selected["__EGL_VENDOR_LIBRARY_FILENAMES"],
                                     str(app / "opengl/share/glvnd/egl_vendor.d/50_mesa.json"))
                    self.assertEqual(selected["LIBGL_DRIVERS_PATH"], str(app / "opengl/lib/dri"))
                    self.assertEqual(selected["GALLIUM_DRIVER"], "llvmpipe")
                    self.assertEqual(selected["LIBGL_ALWAYS_SOFTWARE"], "1")
                    self.assertEqual(selected["VK_DRIVER_FILES"], "/dev/null")
                    self.assertEqual(selected["VK_ICD_FILENAMES"], "/dev/null")

    def test_explicit_gpu_flag_overrides_the_environment(self):
        result, output, _ = self.launch(["--gpu", "ssh://test"], {"NYATERM_RENDERER": "software"}, runtime=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output["env"]["NYATERM_RENDERER"], "gpu")
        self.assertEqual(output["args"], ["ssh://test"])

    def test_separator_forwards_a_literal_launcher_option(self):
        result, output, _ = self.launch(["--", "--software", "two words"], runtime=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output["env"]["NYATERM_RENDERER"], "gpu")
        self.assertEqual(output["args"], ["--software", "two words"])

    def test_bad_mode_or_missing_software_runtime_never_starts_the_app(self):
        for args, settings, code, message in (
            ([], {"NYATERM_RENDERER": "invalid"}, 64, "must be gpu or software"),
            (["--software"], {}, 78, "software renderer is incomplete"),
        ):
            with self.subTest(args=args):
                result, output, _ = self.launch(args, settings, runtime=False)
                self.assertEqual(result.returncode, code)
                self.assertIsNone(output)
                self.assertIn(message, result.stderr)

    def test_child_exit_status_is_preserved_without_relaunching(self):
        for args in ([], ["--software"]):
            with self.subTest(args=args):
                result, output, _ = self.launch(args, {"PROBE_EXIT": "37"})
                self.assertEqual(result.returncode, 37, result.stderr)
                self.assertIsNotNone(output)
                self.assertEqual(len(result.stdout.splitlines()), 1)


if __name__ == "__main__":
    unittest.main()
