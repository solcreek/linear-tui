"""Tests for scripts/check_contract.py, plus the real check of docs/design.md."""

import importlib.util
import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("check_contract", ROOT / "scripts" / "check_contract.py")
cc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cc)

# A two-level tree shaped like `linearctl describe`, small enough to read.
DESCRIBE = {
    "flags": [{"name": "json"}, {"name": "timeout"}],
    "commands": [
        {"name": "issue", "aliases": [], "flags": [], "commands": [
            {"name": "show", "aliases": ["view"], "commands": [],
             "flags": [{"name": "comments"}, {"name": "signed-urls"}]},
            {"name": "list", "aliases": [], "commands": [],
             "flags": [{"name": "limit"}, {"name": "fields"}]},
        ]},
        {"name": "describe", "aliases": [], "flags": [], "commands": []},
    ],
}


def problems(doc):
    return cc.check(doc, DESCRIBE)


class Invocations(unittest.TestCase):
    def test_inline_spans_and_fences(self):
        doc = "Run `linearctl issue list --limit 5` now.\n```\nlinearctl describe\n```\n"
        self.assertEqual([t for _, t in cc.invocations(doc)],
                         ["linearctl issue list --limit 5", "linearctl describe"])

    def test_ignores_other_code(self):
        self.assertEqual(list(cc.invocations("`cargo test` and `ls`\n")), [])

    def test_splits_pipelines(self):
        got = [t for _, t in cc.invocations("`echo x | linearctl describe --json`\n")]
        self.assertEqual(got, ["linearctl describe --json"])

    def test_proposed_section_is_exempt_until_a_sibling_heading(self):
        doc = ("## Gaps (proposed)\n`linearctl watch --json`\n"
               "### detail\n`linearctl serve --stdio`\n"
               "## Next\n`linearctl describe`\n")
        self.assertEqual([t for _, t in cc.invocations(doc)], ["linearctl describe"])

    def test_heading_inside_a_fence_is_not_a_heading(self):
        doc = "## Gaps (proposed)\n```\n# not a heading\nlinearctl watch\n```\n"
        self.assertEqual(list(cc.invocations(doc)), [])


class CheckOne(unittest.TestCase):
    def test_valid(self):
        self.assertEqual(problems("`linearctl issue show ENG-1 --comments=0 --json`"), (1, []))

    def test_alias_resolves(self):
        self.assertEqual(problems("`linearctl issue view ENG-1 --signed-urls 300`")[1], [])

    def test_global_flag_on_a_leaf(self):
        self.assertEqual(problems("`linearctl issue list --timeout 5`")[1], [])

    def test_unknown_flag(self):
        found, p = problems("`linearctl issue list --sort priority`")
        self.assertEqual(found, 1)
        self.assertIn("unknown flag --sort", p[0])

    def test_flag_of_a_sibling_is_rejected(self):
        self.assertIn("unknown flag --comments", problems("`linearctl issue list --comments 3`")[1][0])

    def test_unknown_subcommand(self):
        self.assertIn("unknown command: issue frob", problems("`linearctl issue frob ENG-1`")[1][0])

    def test_unknown_top_level_command(self):
        self.assertIn("unknown command: watch", problems("`linearctl watch --json`")[1][0])

    def test_synopsis_brackets(self):
        self.assertEqual(problems("`linearctl issue list [--limit N] [--fields F]`")[1], [])
        self.assertIn("--bogus", problems("`linearctl issue list [--bogus]`")[1][0])

    def test_unparseable(self):
        self.assertIn("cannot parse", problems("`linearctl issue list \"open`")[1][0])

    def test_group_followed_by_a_flag(self):
        self.assertEqual(problems("`linearctl issue --json`")[1], [])

    def test_bare_global_flags(self):
        self.assertEqual(problems("`linearctl --json`")[1], [])
        self.assertIn("--nope", problems("`linearctl --nope`")[1][0])

    def test_reports_line_numbers(self):
        self.assertTrue(problems("\n\n`linearctl issue list --nope`")[1][0].startswith("line 3:"))


class Main(unittest.TestCase):
    def run_main(self, doc_text, tmp="doc.md"):
        import tempfile, io, contextlib
        with tempfile.TemporaryDirectory() as d:
            doc = Path(d) / tmp
            doc.write_text(doc_text)
            desc = Path(d) / "describe.json"
            desc.write_text(json.dumps(DESCRIBE))
            err = io.StringIO()
            with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
                code = cc.main([str(doc), "--describe", str(desc)])
            return code, err.getvalue()

    def test_ok(self):
        self.assertEqual(self.run_main("`linearctl describe`")[0], 0)

    def test_problem_fails(self):
        code, err = self.run_main("`linearctl describe --nope`")
        self.assertEqual(code, 1)
        self.assertIn("--nope", err)

    def test_a_doc_without_invocations_fails(self):
        self.assertEqual(self.run_main("nothing here")[0], 1)

    def test_help(self):
        import io, contextlib
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(cc.main(["--help"]), 0)
            self.assertEqual(cc.main([]), 2)


class LoadDescribe(unittest.TestCase):
    def test_default_is_the_snapshot(self):
        self.assertEqual(cc.load_describe([])["version"], "0.1.0")

    def test_live_runs_the_binary(self):
        import os, stat, tempfile
        with tempfile.TemporaryDirectory() as d:
            fake = Path(d) / "linearctl"
            fake.write_text("#!/bin/sh\n"
                            "[ \"$1 $2\" = \"describe --json\" ] || exit 2\n"
                            "echo '" + json.dumps(DESCRIBE) + "'\n")
            fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
            self.assertEqual(cc.load_describe(["--live", str(fake)]), DESCRIBE)


class Snapshot(unittest.TestCase):
    def test_snapshot_is_linearctl_0_1_0(self):
        d = json.loads(cc.SNAPSHOT.read_text())
        self.assertEqual(d["version"], "0.1.0")
        self.assertEqual(d["exitCodes"], {"ok": 0, "failed": 1, "usage": 2, "partial": 3,
                                          "busy": 4, "auth": 5, "rateLimited": 6})


class DesignDoc(unittest.TestCase):
    """The real check: every command docs/design.md names exists in linearctl 0.1.0."""

    def test_design_doc_matches_the_contract(self):
        design = ROOT / "docs" / "design.md"
        if not design.exists():
            self.skipTest("docs/design.md not written yet")
        found, p = cc.check(design.read_text(), json.loads(cc.SNAPSHOT.read_text()))
        self.assertGreater(found, 0)
        self.assertEqual(p, [])


if __name__ == "__main__":
    unittest.main()
