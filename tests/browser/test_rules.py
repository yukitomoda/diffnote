"""What the browser tests themselves keep to. Runs no browser."""
import os
import re
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


class TheTestsKeepToTheirRules(unittest.TestCase):
    def test_no_test_waits_a_fixed_time(self):
        """A test waits for what it needs to be so -- `b.wait(...)`, or
        `b.settle()` for "the page has done with what it was doing", or
        `harness.until(...)` for something outside the page -- never for a
        moment. A moment is enough on one machine and not on a slower one,
        which is what made tests fail now and then."""
        for name in sorted(os.listdir(HERE)):
            if not (name.startswith("test_") and name.endswith(".py")) or name == "test_rules.py":
                continue
            with open(os.path.join(HERE, name), encoding="utf-8") as f:
                for n, line in enumerate(f, 1):
                    self.assertIsNone(re.search(r"\bsleep\s*\(", line), f"{name}:{n}: {line.strip()}")

    def test_no_test_navigates_behind_the_harness_back(self):
        """A page is loaded again with `b.reload()` / `b.open()`, which wait
        for the new page: one loaded by the page's own script (`location.
        reload()`) is waited on while the old one is still there, and a
        condition met by the old one passes before the new one has come."""
        for name in sorted(os.listdir(HERE)):
            if not (name.startswith("test_") and name.endswith(".py")) or name == "test_rules.py":
                continue
            with open(os.path.join(HERE, name), encoding="utf-8") as f:
                for n, line in enumerate(f, 1):
                    self.assertIsNone(re.search(r"location\.(reload|assign|replace)\(|location\.href\s*=", line),
                                      f"{name}:{n}: {line.strip()}")


if __name__ == "__main__":
    unittest.main()
