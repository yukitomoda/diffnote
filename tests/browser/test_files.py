"""The "other files" section of the served page: files the diff doesn't touch,
from what the bundle stores and from the git repository."""
import json
import os
import shutil
import unittest

from harness import BrowserCase, Served, entries, make_login_review, show, zip_names

CUR = ".diffnote-revision.is-current"
TREE = f"{CUR} [data-diffnote-tree]"
LIST = f"{CUR} [data-diffnote-tree-list]"
ERROR = f"{CUR} .diffnote-tree .diffnote-error"


def section(path):
    return f"{CUR} section.diffnote-file[data-diffnote-file='{path}']"


def opener(path):
    return f"{CUR} [data-diffnote-open='{path}']"


class FilesCase(BrowserCase):
    def start(self, master, cwd=None, extra=()):
        self.review = os.path.join(self.fresh("review"), "r.diffnote")
        shutil.copy(master, self.review)
        self.server = Served(self.review, cwd=cwd, extra=extra)
        self.addCleanup(self.server.stop)
        self.b = self.browser
        self.b.open(self.server.url)
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")

    def open_tree(self):
        self.b.click(f"{TREE} summary")
        self.assertTrue(self.b.wait(f"document.querySelector('{LIST}').textContent.trim() !== '' && !document.querySelector('{LIST}').textContent.includes('読み込み中')"))

    def search(self, query):
        self.b.js("var s=document.querySelector(%s); s.value=%s; s.dispatchEvent(new Event('input',{bubbles:true}))"
                  % (json.dumps(f"{CUR} .diffnote-tree__search"), json.dumps(query)))

    def open_file(self, path):
        self.search(path)
        self.assertTrue(self.b.wait_exists(opener(path)), path)
        self.b.click(opener(path))
        self.assertTrue(self.b.wait_exists(section(path)), path)

    def listed(self):
        return self.b.js("Array.from(document.querySelectorAll(%s)).map(function(b){return b.getAttribute('data-diffnote-open')})"
                         % json.dumps(f"{LIST} [data-diffnote-open]"))

    def same_page(self):
        return self.b.js("window.__marker==='same-page' && window.__table===document.querySelector('.diffnote-diff')")

    def gutter(self, path, n):
        return f"{section(path)} table tr[data-diffnote-new='{n}'] .diffnote-line__gutter-new"

    def comment_on_line(self, path, n, text):
        b = self.b
        b.click_at(self.gutter(path, n))
        self.assertEqual(b.text(".diffnote-compose__where"), f"{path}:{n}")
        b.set_value(".diffnote-compose textarea", text)
        b.click(".diffnote-compose button[type=submit]")
        self.assertTrue(b.wait_count(f"{section(path)} .diffnote-thread", 1))

    def file_paths(self):
        return self.b.js("Array.from(document.querySelectorAll(%s)).map(function(s){return s.getAttribute('data-diffnote-file')})"
                         % json.dumps(f"{CUR} section.diffnote-file"))


class StoredFiles(FilesCase):
    """A review that stores the whole tree (`--snapshot full`)."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.master, _ = make_login_review(cls.root, snapshot="full", name="stored")

    def test_the_section_is_folded_and_the_list_is_read_only_when_opened(self):
        self.start(self.master)
        b = self.b
        self.assertFalse(b.js(f"document.querySelector('{TREE}').open"))
        self.assertFalse(b.js(f"document.querySelector('{LIST}').hasChildNodes()"))
        self.assertEqual(b.text(f"{TREE} summary"), "その他のファイル")
        self.open_tree()
        root = b.js("Array.from(document.querySelectorAll(%s)).map(function(l){return l.textContent.trim().replace(/\\s+/g,' ')})"
                    % json.dumps(f"{LIST} > .diffnote-tree__list > li"))
        self.assertEqual(root, ["docs/ 2", "src/ 1", "big.txt", "data.bin", "huge.txt"])
        # A directory reads its files when it is opened.
        self.assertFalse(b.exists(f"{LIST} [data-diffnote-dir='docs'] .diffnote-tree__list"))
        b.click(f"{LIST} [data-diffnote-dir='docs'] summary")
        self.assertTrue(b.wait_exists(opener("docs/README.md")))
        self.assertEqual(self.listed(), ["docs/README.md", "docs/設計 メモ.md", "big.txt", "data.bin", "huge.txt"])

    def test_a_search_lists_matching_paths_flat(self):
        self.start(self.master)
        b = self.b
        self.open_tree()
        self.search("設計")
        self.assertTrue(b.wait_exists(opener("docs/設計 メモ.md")))
        self.assertEqual(self.listed(), ["docs/設計 メモ.md"])
        self.assertFalse(b.exists(f"{LIST} [data-diffnote-dir]"))
        self.search("no-such-file")
        self.assertTrue(b.wait(f"document.querySelector('{LIST}').textContent.includes('見つかりません')"))

    def test_opening_a_file_adds_it_to_the_page_and_records_nothing(self):
        self.start(self.master)
        b = self.b
        events, names = entries(self.review), zip_names(self.review)
        self.open_tree()
        self.open_file("docs/README.md")
        self.assertEqual(self.file_paths(), ["src/auth/login.ts", "docs/README.md"], "after the diff's files")
        self.assertTrue(b.js("Array.from(document.querySelectorAll(%s)).some(function(a){return a.textContent==='docs/README.md'})"
                             % json.dumps(f"{CUR} .diffnote-filelist a")))
        self.assertTrue(self.same_page())
        self.assertEqual((entries(self.review), zip_names(self.review)), (events, names), "looking records nothing")

    def test_a_line_and_the_file_can_be_commented_on_and_are_kept(self):
        self.start(self.master)
        b = self.b
        self.open_tree()
        self.open_file("docs/README.md")
        self.comment_on_line("docs/README.md", 3, "この一文を詳しく")
        b.click(f"{section('docs/README.md')} [data-diffnote-add=file]")
        b.set_value(".diffnote-compose textarea", "このファイル全体について")
        b.click(".diffnote-compose button[type=submit]")
        self.assertTrue(b.wait_count(f"{section('docs/README.md')} > details > .diffnote-thread", 1))
        out = show(self.review)
        self.assertIn("docs/README.md:3", out)
        self.assertIn("ファイル全体: docs/README.md", out)
        # After a reload only what has a thread remains (as an ordinary file).
        b.reload()
        self.assertEqual(self.file_paths(), ["src/auth/login.ts", "docs/README.md"])
        self.open_tree()
        self.search("README")
        self.assertTrue(b.wait(f"document.querySelector('{LIST}').textContent.includes('見つかりません')"),
                        "no longer offered as another file")

    def test_a_long_file_is_shown_in_chunks_and_a_later_line_can_be_commented_on(self):
        self.start(self.master)
        b = self.b
        self.open_tree()
        self.open_file("big.txt")
        rows = f"{section('big.txt')} tr[data-diffnote-new]"
        more = f"{section('big.txt')} [data-diffnote-more]"
        self.assertEqual(b.count(rows), 500)
        self.assertEqual(b.text(more), "続きを表示(501〜 / 全 1200 行)")
        b.click(more)
        self.assertTrue(b.wait_count(rows, 1000))
        b.click(more)
        self.assertTrue(b.wait_gone(more))
        self.assertEqual(b.count(rows), 1200)
        self.comment_on_line("big.txt", 750, "750 行目")
        self.assertIn("big.txt:750", show(self.review))

    def test_closing_a_file_removes_it_and_records_nothing(self):
        self.start(self.master)
        b = self.b
        self.open_tree()
        self.open_file("src/util/b.ts")
        events = entries(self.review)
        b.click(f"{section('src/util/b.ts')} [data-diffnote-close]")
        self.assertFalse(b.exists(section("src/util/b.ts")))
        self.assertFalse(b.js("Array.from(document.querySelectorAll(%s)).some(function(a){return a.textContent==='src/util/b.ts'})"
                              % json.dumps(f"{CUR} .diffnote-filelist a")))
        self.assertEqual(entries(self.review), events)

    def test_a_file_that_cannot_be_shown_is_refused_with_a_reason(self):
        self.start(self.master)
        b = self.b
        self.open_tree()
        self.search("data")
        self.assertTrue(b.wait_exists(opener("data.bin")))
        b.click(opener("data.bin"))
        self.assertTrue(b.wait_exists(ERROR))
        self.assertEqual(b.text(ERROR), "テキストファイルではないため、表示できません")


class FilesFromGit(FilesCase):
    """A review made from git that stores only what it needs (the default)."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.master, cls.repo = make_login_review(cls.root, name="fromgit")

    def test_the_rest_of_the_commit_is_listed_and_opened_from_the_repository(self):
        self.start(self.master, cwd=self.repo)
        self.assertEqual(self.server.notices, [])
        events, names = entries(self.review), zip_names(self.review)
        self.open_tree()
        self.open_file("docs/README.md")
        self.assertEqual((entries(self.review), zip_names(self.review)), (events, names), "looking records nothing")

    def test_a_comment_stores_the_file_with_the_thread(self):
        self.start(self.master, cwd=self.repo)
        events, names = entries(self.review), zip_names(self.review)
        self.open_tree()
        self.open_file("docs/README.md")
        self.comment_on_line("docs/README.md", 3, "git から開いたファイル")
        self.assertEqual(len(zip_names(self.review)), len(names) + 1, "the file's content")
        self.assertEqual(entries(self.review), events + 2, "its entry for the revision, and the thread")
        self.assertIn("[固定]", show(self.review))
        self.assertIn("docs/README.md:3", show(self.review))

    def test_too_big_files_and_binary_files_are_refused_with_a_reason(self):
        self.start(self.master, cwd=self.repo)
        b = self.b
        self.open_tree()
        for path, reason in (("huge.txt", "大きすぎるため開けません(2.9 MB。上限は 2 MB)"),
                             ("data.bin", "テキストファイルではないため、表示できません")):
            self.search(path)
            self.assertTrue(b.wait_exists(opener(path)), path)
            b.js("var e=document.querySelector(%s); e && e.remove()" % json.dumps(ERROR))
            b.click(opener(path))
            self.assertTrue(b.wait_exists(ERROR), path)
            self.assertEqual(b.text(ERROR), reason)

    def test_a_long_file_comes_from_git_in_chunks(self):
        self.start(self.master, cwd=self.repo)
        b = self.b
        self.open_tree()
        self.open_file("big.txt")
        b.click(f"{section('big.txt')} [data-diffnote-more]")
        self.assertTrue(b.wait_count(f"{section('big.txt')} tr[data-diffnote-new]", 1000))

    def test_the_repository_can_be_named_when_started_elsewhere(self):
        self.start(self.master, cwd=self.root, extra=("--repo", self.repo))
        self.assertEqual(self.server.notices, [])
        self.open_tree()
        self.open_file("docs/README.md")

    def test_without_the_repository_only_stored_files_open_and_the_page_says_so(self):
        self.start(self.master, cwd=self.root)
        self.assertEqual(len(self.server.notices), 1)
        self.assertIn("git リポジトリの中で起動していない", self.server.notices[0])
        self.open_tree()
        self.assertTrue(self.b.wait(f"document.querySelector('{LIST}').textContent.includes('リポジトリが見つからない')"))
        self.assertEqual(self.listed(), [])


if __name__ == "__main__":
    unittest.main()
