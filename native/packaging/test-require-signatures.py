#!/usr/bin/env python3
"""A public release must never pass with any missing signing proof."""
import itertools
import os
from pathlib import Path
import subprocess
import unittest

SCRIPT = Path(__file__).with_name("require-signatures.sh")
FIELDS = ["WINDOWS_SIGNED", "MACOS_SIGNED", "SDK_WINDOWS_SIGNED",
          "SDK_MACOS_SIGNED", "LINUX_GPG_SIGNED"]


class SigningGate(unittest.TestCase):
    def run_gate(self, proofs, draft="false"):
        env = {key: value for key, value in os.environ.items() if key not in FIELDS}
        env.update(dict(zip(FIELDS, proofs)))
        env["DRAFT_ONLY"] = draft
        return subprocess.run(["bash", str(SCRIPT)], env=env, text=True,
                              capture_output=True)

    def test_all_signing_combinations(self):
        for proofs in itertools.product(["false", "true"], repeat=len(FIELDS)):
            with self.subTest(proofs=proofs):
                public = self.run_gate(proofs)
                self.assertEqual(public.returncode == 0, all(v == "true" for v in proofs))
                draft = self.run_gate(proofs, "true")
                self.assertEqual(draft.returncode, 0)
                if "false" in proofs:
                    self.assertIn("::error::", public.stdout)
                    self.assertIn("::warning::", draft.stdout)

    def test_missing_or_malformed_proof_is_refused(self):
        for missing in ["", "TRUE", "1", "unknown"]:
            self.assertNotEqual(self.run_gate(["true"] * 4 + [missing]).returncode, 0)

    def test_only_explicit_draft_can_relax_gate(self):
        self.assertNotEqual(self.run_gate(["false"] * 5, "TRUE").returncode, 0)


if __name__ == "__main__":
    unittest.main()
