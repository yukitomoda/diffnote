"""`diffnote serve`'s page: replies,
resolving, the version check, shutting down."""
import unittest

import base64
import json
import time

import harness
from harness import BrowserCase, Served, add_settings, entries, make_calc_review, make_gaps_review, make_indent_review, make_login_review, show
import os
import pathlib
import shutil
import subprocess
import sys

CUR = ".diffnote-revision.is-current"


class ServedCase(BrowserCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.calc, _ = make_calc_review(cls.root)
        cls.login, _ = make_login_review(cls.root)

    def serve(self, master=None):
        self.review = os.path.join(self.fresh("review"), "r.diffnote")
        shutil.copy(master or self.calc, self.review)
        self.server = Served(self.review, author="検証者")
        self.addCleanup(self.server.stop)
        self.b = self.browser
        self.b.open(self.server.url)
        self.b.js("localStorage.setItem('diffnote-hide-resolved','0'); localStorage.setItem('diffnote-layout','unified')")
        self.b.reload()
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")

    def write(self, selector, text):
        """Types into a box the way the page hears it (and lets it settle)."""
        self.b.js("var t=document.querySelector(%r); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(t,%r); t.dispatchEvent(new Event('input',{bubbles:true}))" % (selector, text))
        time.sleep(0.1)

    def paste(self, selector, name, mime, content, base64=False):
        """Pastes a file into a box, as a screenshot from the clipboard arrives."""
        self.b.js("""(function(sel, name, mime, content, base64){
          var bytes = base64 ? Uint8Array.from(atob(content), function (c) { return c.charCodeAt(0); }) : new TextEncoder().encode(content);
          var dt = new DataTransfer(); dt.items.add(new File([bytes], name, {type: mime}));
          var ta = document.querySelector(sel); ta.focus();
          ta.dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
        })(%s, %s, %s, %s, %s)""" % (json.dumps(selector), json.dumps(name), json.dumps(mime), json.dumps(content), "true" if base64 else "false"))

    def reply_to(self, card, text, shows=None):
        b = self.b
        b.js(f"var t=document.getElementById({card!r}).querySelector('textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(t,{text!r}); t.dispatchEvent(new Event('input',{{bubbles:true}}))")
        time.sleep(0.1)
        b.js(f"document.getElementById({card!r}).querySelector('form.diffnote-reply').requestSubmit()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes({(shows or text)!r}) && !document.querySelector('.is-pending')"))

    def same_page(self):
        return self.b.js("window.__marker==='same-page' && window.__table===document.querySelector('.diffnote-diff')")

    def card(self, text):
        return self.b.js("""(function(){var c=Array.from(document.querySelectorAll('%s .diffnote-thread')).find(function(t){return t.textContent.includes(%r)}); return c.id})()""" % (CUR, text))

    def counts(self):
        # From the thread list at the side: "open / all" in its badge, and the
        # threads it marks as resolved.
        return self.b.js("""(() => {
            const all = document.querySelector('.diffnote-threadlist').closest('details').querySelector('.diffnote-badge').textContent.split('/')[1].trim();
            const done = document.querySelectorAll('.diffnote-threadlist li.is-resolved').length;
            return `スレッド ${all} 件(解決済み ${done} 件)`;
        })()""")

    def user_config_file(self):
        return os.path.join(harness.USER_CONFIG_DIR, "config.json")

    def settings_section(self):
        return self.b.js("(function(){var p=document.querySelector('[data-diffnote-settings-page]'); return p && p.dataset.diffnoteSettingsSection})()")

    def set_author_via_user_settings(self, name):
        """Opens the settings screen from the sidebar chip (which goes straight
        to its ユーザー設定 section), sets the author name and saves, then goes
        back to the review."""
        b = self.b
        b.click("[data-diffnote-user-settings]")
        self.assertTrue(b.wait("(function(){var p=document.querySelector('[data-diffnote-settings-page]'); return p && p.dataset.diffnoteSettingsSection === 'user'})()"))
        b.set_value("[data-diffnote-user-setting-author]", name)
        b.click("[data-diffnote-user-settings-save]")
        self.assertTrue(b.wait_exists("[data-diffnote-user-settings-saved]"))
        b.click("[data-diffnote-settings-back]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))


class Replies(ServedCase):
    def test_a_reply_shows_at_once_and_is_kept(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        b.js(f"var c=document.getElementById({card!r}); var t=c.querySelector('textarea'); var set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set; set.call(t,'返信のテスト'); t.dispatchEvent(new Event('input',{{bubbles:true}}))")
        b.js(f"document.getElementById({card!r}).querySelector('form.diffnote-reply').requestSubmit()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('返信のテスト') && !document.querySelector('.is-pending')"))
        self.assertTrue(self.same_page())
        authors = b.js(f"Array.from(document.getElementById({card!r}).querySelectorAll('.diffnote-comment__author')).map(function(a){{return a.textContent}})")
        self.assertTrue(authors[-1].startswith("検証者"), authors)
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('textarea').value"), "")
        self.assertIn("返信のテスト", show(self.review))

    def test_resolving_and_reopening_change_the_card_and_the_counts(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 1 件)")
        written = entries(self.review)
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=reopen]')"))
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 2 件)")
        self.assertTrue(self.same_page())
        deadline = time.time() + 8
        while entries(self.review) == written and time.time() < deadline:
            time.sleep(0.05)
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=resolve]')"))
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 1 件)")
        self.assertIn("再オープン", show(self.review))

    def test_pressing_resolve_twice_quickly_ends_as_the_last_press_says(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        written = entries(self.review)
        # The second press as soon as the page shows the first, before the answer.
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=reopen]')"))
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        deadline = time.time() + 8
        while entries(self.review) < written + 2 and time.time() < deadline:
            time.sleep(0.05)
        time.sleep(0.3)
        self.assertEqual(self.counts(), "スレッド 4 件(解決済み 1 件)", "resolved then reopened: as it was")
        self.assertTrue(b.js(f"!!document.getElementById({card!r}).querySelector('[data-diffnote-action=resolve]')"))

    def test_a_failed_change_says_so_and_keeps_the_draft(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        b.js(f"var t=document.getElementById({card!r}).querySelector('textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(t,'止まった後'); t.dispatchEvent(new Event('input',{{bubbles:true}}))")
        self.server.shut_down(b)
        b.js(f"document.getElementById({card!r}).querySelector('form.diffnote-reply').requestSubmit()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('.diffnote-error')"))
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('.diffnote-error').textContent"), "サーバーに接続できませんでした")
        self.assertEqual(b.js(f"document.getElementById({card!r}).querySelector('textarea').value"), "止まった後")

    def outside_change(self, card):
        """A change the page doesn't know of (as from another tab)."""
        thread = self.b.js(f"document.getElementById({card!r}).dataset.diffnoteThreadId")
        self.b.js("Diffnote.api.post('/api/threads/%s/replies',{body:'別のタブから'})" % thread)

    def test_coming_back_to_the_window_shows_what_changed_meanwhile(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.outside_change(card)
        self.assertFalse(b.js(f"document.getElementById({card!r}).textContent.includes('別のタブから')"))
        b.js("window.dispatchEvent(new Event('focus'))")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('別のタブから')"))
        self.assertTrue(self.same_page())

    def test_a_change_made_over_a_stale_page_brings_the_rest_along(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.outside_change(card)
        b.js("var ids=Array.from(document.querySelectorAll('%s .diffnote-thread')).map(function(t){return t.id}); window.__other=ids.find(function(i){return i!==%r})" % (CUR, card))
        b.js("var c=document.getElementById(window.__other); c.querySelector('[data-diffnote-action]').click()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('別のタブから')"))

    def test_the_export_button_gives_the_page_the_export_command_writes(self):
        self.serve()
        b = self.b
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-export]"))
        self.assertEqual(b.js("document.querySelector('[data-diffnote-export]').getAttribute('href')"), "/export")
        # No `download` attribute: with a bare one the page (Preact) made it
        # `download="true"` and the browser saved the file as "true". The
        # name comes from the server's Content-Disposition.
        self.assertFalse(b.js("document.querySelector('[data-diffnote-export]').hasAttribute('download')"))
        # What the link fetches: an attachment named after the bundle, with the
        # comments in it and nothing that talks to the server.
        b.js("fetch('/export',{credentials:'same-origin'}).then(function(r){return r.text().then(function(t){window.__export={disposition:r.headers.get('content-disposition'),text:t}})})")
        self.assertTrue(b.wait("!!window.__export"))
        self.assertIn('filename="r.html"', b.js("window.__export.disposition"))
        self.assertTrue(b.js("window.__export.text.includes('mul の型') && !window.__export.text.includes('D.api = ')"))

    def test_the_download_button_gives_the_bundle_exactly_as_it_is(self):
        self.serve()
        b = self.b
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-download]"))
        self.assertEqual(b.js("document.querySelector('[data-diffnote-download]').getAttribute('href')"), "/download")
        self.assertFalse(b.js("document.querySelector('[data-diffnote-download]').hasAttribute('download')"))
        b.js("fetch('/download',{credentials:'same-origin'}).then(function(r){return r.arrayBuffer().then(function(buf){"
             "window.__download={disposition:r.headers.get('content-disposition'), type:r.headers.get('content-type'), size:buf.byteLength}})})")
        self.assertTrue(b.wait("!!window.__download"))
        self.assertIn('filename="r.diffnote"', b.js("window.__download.disposition"))
        self.assertEqual(b.js("window.__download.type"), "application/zip")
        self.assertEqual(b.js("window.__download.size"), os.path.getsize(self.review))
        # A reply made through the session is in it (the file on disk, byte for byte).
        b.click("[data-diffnote-settings-back]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))
        card = self.card("mul の型")
        self.reply_to(card, "ダウンロードの確認")
        b.js("fetch('/download',{credentials:'same-origin'}).then(function(r){return r.arrayBuffer().then(function(buf){window.__size2=buf.byteLength})})")
        self.assertTrue(b.wait("window.__size2 !== undefined && window.__size2 !== window.__download.size"))
        self.assertEqual(b.js("window.__size2"), os.path.getsize(self.review))

    def test_what_a_comment_says_is_text_and_markdown_never_html_or_script(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        body = '<img src=x onerror="window.__ran=1"> **強調** [悪い](javascript:window.__ran=2) [良い](https://example.com/a) `code`'
        self.reply_to(card, body, shows="良い")
        mine = "[data-diffnote-mine] .diffnote-comment__body"
        self.assertEqual(b.count(f"{mine} img, {mine} script"), 0, "no element from raw HTML")
        self.assertIn('<img src=x onerror="window.__ran=1">', b.text(mine), "it is shown as the text it is")
        self.assertEqual(b.count(f"{mine} strong"), 1)
        self.assertEqual(b.count(f"{mine} code"), 1)
        self.assertEqual(b.count(f"{mine} a"), 1, "only the safe link is a link")
        self.assertEqual(b.js(f"document.querySelector('{mine} a').getAttribute('href')"), "https://example.com/a")
        self.assertIn("悪い", b.text(mine))
        time.sleep(0.3)
        self.assertFalse(b.js("'__ran' in window"))

    def test_every_comment_has_a_menu_and_only_those_of_the_signed_in_name_are_marked_as_mine(self):
        self.serve()
        b = self.b
        total = b.count("[data-diffnote-comment]")
        self.assertEqual(b.count("[data-diffnote-comment-menu]"), total, "the older ones too")
        self.assertEqual(b.count("[data-diffnote-mine]"), 0, "nothing of 検証者 yet")
        card = self.card("mul の型")
        self.reply_to(card, "あとから書いた返信")
        self.assertEqual(b.count("[data-diffnote-mine]"), 1)
        self.assertIn("あとから書いた返信", b.text("[data-diffnote-mine]"))
        # The menu is shut until it is opened.
        panel = "[data-diffnote-mine] .diffnote-comment__panel"
        self.assertTrue(b.js(f"document.querySelector({json.dumps(panel)}).hidden"))
        b.click("[data-diffnote-mine] [data-diffnote-comment-menu]")
        self.assertFalse(b.js(f"document.querySelector({json.dumps(panel)}).hidden"))

    def test_a_comment_of_another_name_is_asked_about_before_it_is_edited_or_deleted(self):
        self.serve()
        b = self.b
        others = "[data-diffnote-comment]:not([data-diffnote-mine])"
        card = self.card("mul の型")
        first = f"#{card} {others}"
        self.assertTrue(b.exists(first))
        # Edit: a warning first, and nothing changes if it is declined.
        b.click(f"{first} [data-diffnote-edit]")
        self.assertTrue(b.wait_exists(f"{first} [data-diffnote-warn]"))
        warning = b.text(f"{first} [data-diffnote-warn]")
        self.assertIn("reviewer", warning)
        self.assertIn("検証者", warning)
        self.assertFalse(b.exists("[data-diffnote-edit-form]"))
        b.click(f"{first} [data-diffnote-warn-cancel]")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(first + ' [data-diffnote-warn]')})"))
        # Agreed: it can be edited, whoever wrote it and whenever.
        b.click(f"{first} [data-diffnote-edit]")
        self.assertTrue(b.wait_exists(f"{first} [data-diffnote-warn]"))
        b.click(f"{first} [data-diffnote-warn-ok]")
        self.assertTrue(b.wait_exists("[data-diffnote-edit-form] textarea"))
        self.write("[data-diffnote-edit-form] textarea", "型は number を想定します。")
        b.js("document.querySelector('[data-diffnote-edit-form]').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-edit-form]')"))
        out = show(self.review)
        self.assertIn("型は number を想定します。", out)
        self.assertNotIn("mul の型を確認してください。", out)

    def test_deleting_the_first_comment_of_a_thread_deletes_only_that_comment(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "自分の返信")
        b.click(f"#{card} [data-diffnote-comment]:not([data-diffnote-mine]) [data-diffnote-delete]")
        self.assertTrue(b.wait_exists(f"#{card} [data-diffnote-warn]"))
        warning = b.text(f"#{card} [data-diffnote-warn]")
        self.assertIn("reviewer", warning, "whose it is")
        self.assertIn("このコメントだけが削除されます", warning)
        self.assertIn("返信 1 件は残り", warning)
        self.assertIn("mul の型", show(self.review), "nothing is deleted before it is agreed")
        b.click(f"#{card} [data-diffnote-warn-ok]")
        self.assertTrue(b.wait(f"!!document.querySelector('#{card} [data-diffnote-deleted]')"))
        self.assertEqual(b.text(f"#{card} [data-diffnote-deleted]"), "このコメントは削除されました")
        out = show(self.review)
        self.assertNotIn("mul の型を確認してください。", out)
        self.assertIn("自分の返信", out, "the reply stays")
        self.assertTrue(b.js(f"!!document.getElementById({card!r})"), "so does the thread")
        # What was deleted has no menu, and the list still tells the thread by its reply.
        self.assertEqual(b.count(f"#{card} [data-diffnote-deleted] ~ [data-diffnote-comment-menu]"), 0)
        self.assertTrue(b.js("[...document.querySelectorAll('.diffnote-threadlist__preview')].some(e => e.textContent.includes('自分の返信'))"))
        # Deleting the reply too takes the whole thread with it.
        b.click(f"#{card} [data-diffnote-mine] [data-diffnote-delete]")
        b.click(f"#{card} [data-diffnote-warn-ok]")
        self.assertTrue(b.wait(f"!document.getElementById({card!r})"))

    def test_what_this_session_added_or_edited_is_tinted_faintly(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.assertEqual(b.count("[data-diffnote-changed]"), 0, "nothing of this session yet")
        self.reply_to(card, "追加した")
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-changed]').length === 1"))
        # It stays after the page is loaded again.
        b.js("location.reload()")
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-changed]').length === 1"))
        self.assertNotEqual(b.js("getComputedStyle(document.querySelector('[data-diffnote-changed]')).backgroundColor"), "rgba(0, 0, 0, 0)")

    def test_a_comment_of_this_session_can_be_edited(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "書き間違えた")
        b.click("[data-diffnote-mine] [data-diffnote-edit]")
        self.assertTrue(b.wait_exists("[data-diffnote-edit-form] textarea"))
        self.assertEqual(b.value("[data-diffnote-edit-form] textarea"), "書き間違えた", "the text as written")
        self.write("[data-diffnote-edit-form] textarea", "書き直した")
        b.js("document.querySelector('[data-diffnote-edit-form]').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-edit-form]') && document.body.textContent.includes('書き直した')"))
        self.assertFalse(b.js("document.body.textContent.includes('書き間違えた')"))
        self.assertTrue(self.same_page())
        out = show(self.review)
        self.assertIn("書き直した", out)
        self.assertNotIn("書き間違えた", out)
        # Cancelling leaves it alone.
        b.click("[data-diffnote-mine] [data-diffnote-edit]")
        self.assertTrue(b.wait_exists("[data-diffnote-edit-form]"))
        self.write("[data-diffnote-edit-form] textarea", "やっぱりやめた")
        b.js("Array.from(document.querySelectorAll('[data-diffnote-edit-form] button')).find(function(x){return x.textContent==='キャンセル'}).click()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-edit-form]')"))
        self.assertIn("書き直した", show(self.review))

    def test_a_reply_of_this_session_can_be_deleted_after_confirming(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "消したい返信")
        mine = "[data-diffnote-mine]"
        b.click(f"{mine} [data-diffnote-delete]")
        self.assertTrue(b.wait_exists(f"{mine} [data-diffnote-warn]"), "asked first")
        b.click(f"{mine} [data-diffnote-warn-cancel]")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(mine + ' [data-diffnote-warn]')})"))
        self.assertIn("消したい返信", show(self.review), "declined: still there")
        b.click(f"{mine} [data-diffnote-delete]")
        self.assertTrue(b.wait_exists(f"{mine} [data-diffnote-warn-ok]"))
        b.click(f"{mine} [data-diffnote-warn-ok]")
        self.assertTrue(b.wait("!document.body.textContent.includes('消したい返信')"))
        self.assertNotIn("消したい返信", show(self.review))
        self.assertEqual(b.count("[data-diffnote-mine]"), 0)
        self.assertTrue(b.js(f"!!document.getElementById({card!r})"), "the thread stays")

    def test_a_new_thread_can_be_taken_out_again_with_what_it_was_given(self):
        self.serve()
        b = self.b
        b.click(f"{CUR} [data-diffnote-add=global]")
        self.write(".diffnote-compose textarea", "やっぱり要らない全体コメント")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("document.body.textContent.includes('やっぱり要らない全体コメント') && !document.querySelector('.diffnote-compose-wrap')"))
        threads = self.counts()
        b.click("[data-diffnote-global] [data-diffnote-mine] [data-diffnote-delete]")
        self.assertTrue(b.wait_exists("[data-diffnote-global] [data-diffnote-warn-ok]"))
        b.click("[data-diffnote-global] [data-diffnote-warn-ok]")
        self.assertTrue(b.wait("!document.body.textContent.includes('やっぱり要らない全体コメント')"))
        self.assertNotEqual(self.counts(), threads)
        self.assertNotIn("やっぱり要らない全体コメント", show(self.review))

    def test_two_servers_at_once_keep_working_in_the_same_browser(self):
        self.serve()
        first = self.server
        other_review = os.path.join(self.fresh("review"), "other.diffnote")
        shutil.copy(self.calc, other_review)
        second = Served(other_review, author="二つ目")
        self.addCleanup(second.stop)
        b = self.b
        # Visiting the second server would, with one shared cookie, log the first out.
        b.open(second.url)
        b.open(first.url.split("/?")[0] + "/")
        self.assertTrue(b.exists(".diffnote-diff"), "the first server still answers")
        b.open(second.url.split("/?")[0] + "/")
        self.assertTrue(b.exists(".diffnote-diff"), "and so does the second")
        # A reply to the first is still accepted.
        card = self.card("mul の型")
        b.open(first.url.split("/?")[0] + "/")
        card = self.card("mul の型")
        self.reply_to(card, "二つのサーバーの間で")
        self.assertIn("二つのサーバーの間で", show(self.review))

    def test_the_screen_is_a_page_of_the_review_laid_out_as_the_review_is(self):
        self.serve()
        b = self.b
        # Where the diff's own column starts, before leaving the review.
        diff_left = b.js("document.querySelector('%s section.diffnote-file').getBoundingClientRect().left" % CUR)
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page]"))
        got = b.js("""(() => {
          const nav = document.querySelector('.diffnote-settings-nav');
          const pane = document.querySelector('.diffnote-settings__pane');
          return { pane: pane.getBoundingClientRect().left,
                   width: pane.getBoundingClientRect().width,
                   sticky: getComputedStyle(nav).position,
                   groups: nav.querySelectorAll('ul').length,
                   order: [...nav.querySelectorAll('[data-diffnote-settings-nav]')]
                     .map(b => b.dataset.diffnoteSettingsNav) };
        })()""")
        self.assertAlmostEqual(got["pane"], diff_left, delta=1,
                               msg="the same column as the review, so nothing moves")
        self.assertGreater(got["width"], 500, "and room to put a list in")
        self.assertEqual(got["sticky"], "sticky", "the nav stays while a long pane scrolls")
        self.assertEqual(got["groups"], 2, "what the review holds, then what is set")
        self.assertEqual(got["order"], ["timeline", "attachments", "general", "settings", "user"])

    def open_settings(self):
        """Opens the settings screen and switches to its 設定 section (the
        screen itself starts on 全般; see test_the_title_is_the_way_into..."""
        b = self.b
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page]"))
        b.click("[data-diffnote-settings-nav='settings']")
        self.assertTrue(b.wait_exists("[data-diffnote-setting-title]"))

    def close_settings(self):
        b = self.b
        b.click("[data-diffnote-settings-back]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))

    def save_settings(self):
        b = self.b
        b.click("[data-diffnote-settings-save]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-saved]"))

    def test_the_title_is_the_way_into_the_settings_screen_and_they_are_kept_in_the_review(self):
        self.serve()
        b = self.b
        self.assertIn("diffnote レビュー", b.text("[data-diffnote-settings]"))
        self.assertFalse(b.exists("[data-diffnote-inline=title]"), "the old pencil is gone")
        # The title opens the screen, starting on 全般 (bundle info, actions).
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page]"))
        # A screen of its own: the review is out of the way (and not taken down).
        self.assertTrue(b.js("document.querySelector('.diffnote-review-body').hidden"))
        self.assertEqual(b.js("document.querySelector('[data-diffnote-settings]').getAttribute('aria-pressed')"), "true")
        self.assertEqual(self.settings_section(), "general")
        info = b.text("[data-diffnote-bundle-info]")
        self.assertIn("リビジョン2 件", info)
        self.assertIn("画像0 件", info)
        # 設定 has the form.
        b.click("[data-diffnote-settings-nav='settings']")
        self.assertTrue(b.wait_exists("[data-diffnote-setting-title]"))
        self.assertEqual(self.settings_section(), "settings")
        self.assertEqual(b.value("[data-diffnote-setting-title]"), "")
        self.assertEqual(b.value("[data-diffnote-setting-limit]"), "5")
        self.assertFalse(b.js("document.querySelector('[data-diffnote-setting-ignore]').checked"))
        # Nothing to save until something is changed; and nothing is kept if it is left.
        self.assertTrue(b.js("document.querySelector('[data-diffnote-settings-save]').disabled"))
        b.set_value("[data-diffnote-setting-title]", "新しいタイトル")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-dirty]"), "it says so")
        self.close_settings()
        self.assertFalse(b.js("document.querySelector('.diffnote-review-body').hidden"))
        self.assertNotIn("新しいタイトル", show(self.review))
        self.assertIn("diffnote レビュー", b.text("[data-diffnote-settings]"))
        # Saved: the screen stays, and says so.
        self.open_settings()
        self.assertEqual(b.value("[data-diffnote-setting-title]"), "", "starts again from what is kept")
        b.set_value("[data-diffnote-setting-title]", "新しいタイトル")
        b.js("document.querySelector('[data-diffnote-setting-ignore]').click()")
        b.set_value("[data-diffnote-setting-limit]", "2")
        self.save_settings()
        self.assertIn("新しいタイトル", b.text("[data-diffnote-settings]"))
        self.assertTrue(self.same_page())
        out = show(self.review)
        self.assertIn("[設定] タイトル=新しいタイトル", out)
        self.assertIn("[設定] 空白の違いを無視(初期表示)", out)
        self.assertIn(f"[設定] 添付ファイルの上限={2 * 1024 * 1024} バイト", out)
        self.assertTrue(b.js("document.querySelector('[data-diffnote-settings-save]').disabled"), "nothing more to save")
        # Back at the review: what it was told to start from does not change the page it is on.
        self.close_settings()
        self.assertFalse(b.js("document.querySelector('[data-diffnote-ignore-space]').checked"))
        # Esc leaves the screen too, and the limit is what was saved.
        self.open_settings()
        self.assertEqual(b.value("[data-diffnote-setting-limit]"), "2")
        time.sleep(0.2)  # (the screen listens for Escape once it has been drawn)
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"), "Escape leaves it")
        # Emptied: the default heading comes back.
        self.open_settings()
        b.set_value("[data-diffnote-setting-title]", "")
        self.save_settings()
        self.assertIn("diffnote レビュー", b.text("[data-diffnote-settings]"))
        self.assertNotIn("タイトル=", show(self.review))

    def test_what_was_being_written_is_still_there_after_a_visit_to_the_settings(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, "書きかけの返信")
        self.open_settings()
        self.assertFalse(b.js("document.querySelector('[data-diffnote-settings-page]') === null"))
        self.close_settings()
        self.assertEqual(b.value(box), "書きかけの返信")
        self.assertTrue(self.same_page(), "the review was not built again")
        # A tab leaves the settings for the review too.
        self.open_settings()
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))

    def test_a_setting_that_is_not_a_number_or_out_of_range_is_told_and_nothing_is_kept(self):
        self.serve()
        b = self.b
        self.open_settings()
        b.set_value("[data-diffnote-setting-limit]", "")
        b.click("[data-diffnote-settings-save]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page] .diffnote-error"))
        self.assertIn("数字", b.text("[data-diffnote-settings-page] .diffnote-error"))
        b.set_value("[data-diffnote-setting-limit]", "500")
        b.click("[data-diffnote-settings-save]")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-settings-page] .diffnote-error').textContent.includes('指定してください')"))
        self.assertTrue(b.exists("[data-diffnote-settings-page]"), "stays to be put right")
        self.assertNotIn("上限", show(self.review))

    def test_a_file_over_the_limit_that_was_set_is_refused_on_the_page(self):
        self.serve()
        b = self.b
        self.open_settings()
        b.set_value("[data-diffnote-setting-limit]", "1")
        self.save_settings()
        self.close_settings()
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        b.js("""(function(sel){
          var dt = new DataTransfer(); dt.items.add(new File([new Uint8Array(1200 * 1024)], 'over.bin', {type: 'application/octet-stream'}));
          var ta = document.querySelector(sel); ta.focus();
          ta.dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
        })(%s)""" % json.dumps(box))
        self.assertTrue(b.wait("!!document.querySelector('.is-failed[data-diffnote-attach-status]')"))
        self.assertIn("1 MB", b.text("[data-diffnote-attach-status]"))

    def test_the_whole_chip_is_the_button_the_picture_of_the_name_included(self):
        self.serve()
        b = self.b
        got = b.js("""(() => {
          const button = document.querySelector('[data-diffnote-user-settings]');
          const avatar = document.querySelector('.diffnote-user__avatar');
          const r = button.getBoundingClientRect(), a = avatar.getBoundingClientRect();
          const middle = document.elementFromPoint((a.left + a.right) / 2, (a.top + a.bottom) / 2);
          return { inside: button.contains(avatar),
                   covered: a.left >= r.left - 1 && a.right <= r.right + 1 && a.top >= r.top - 1 && a.bottom <= r.bottom + 1,
                   pressed: middle.closest('[data-diffnote-user-settings]') !== null };
        })()""")
        self.assertEqual(got, {"inside": True, "covered": True, "pressed": True}, got)
        # And pressing it there opens the screen, as pressing the name does.
        b.click(".diffnote-user__avatar")
        self.assertTrue(b.wait_exists("[data-diffnote-user-setting-author]"))
        self.assertEqual(self.settings_section(), "user")

    def test_the_user_chip_opens_a_screen_that_sets_the_author_for_every_review(self):
        self.serve()
        b = self.b
        self.addCleanup(lambda: os.path.exists(self.user_config_file()) and os.remove(self.user_config_file()))
        self.assertEqual(b.text("[data-diffnote-author]"), "検証者", "what serve() started it with")
        # Nothing configured yet: the field starts empty. The chip goes straight
        # to ユーザー設定 (not 全般, where the title button lands).
        b.click("[data-diffnote-user-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-user-setting-author]"))
        self.assertEqual(self.settings_section(), "user")
        self.assertEqual(b.value("[data-diffnote-user-setting-author]"), "")
        self.assertTrue(b.wait("document.activeElement === document.querySelector('[data-diffnote-user-setting-author]')"), "focused")
        self.assertTrue(b.js("document.querySelector('[data-diffnote-user-settings-save]').disabled"), "nothing changed yet")
        b.click("[data-diffnote-settings-back]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))
        self.set_author_via_user_settings("別の人")
        self.assertEqual(b.text("[data-diffnote-author]"), "別の人", "used for the rest of this session too")
        card = self.card("mul の型")
        self.reply_to(card, "名前を変えたあとの返信")
        authors = b.js(f"Array.from(document.getElementById({card!r}).querySelectorAll('.diffnote-comment__author')).map(function(a){{return a.textContent}})")
        self.assertTrue(authors[-1].startswith("別の人"), authors)
        # Kept on this machine: a new server, with nothing else set, starts with it.
        self.server.stop()
        self.server = harness.Served(self.review, author=None)
        self.addCleanup(self.server.stop)
        b.open(self.server.url, ready="!!document.querySelector('.diffnote-file')")
        self.assertEqual(b.text("[data-diffnote-author]"), "別の人")

    def test_too_long_an_author_name_is_refused_and_a_blank_one_clears_it(self):
        self.serve()
        b = self.b
        self.addCleanup(lambda: os.path.exists(self.user_config_file()) and os.remove(self.user_config_file()))
        self.set_author_via_user_settings("別の人")
        b.click("[data-diffnote-user-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-user-setting-author]"))
        b.set_value("[data-diffnote-user-setting-author]", "あ" * 101)
        b.click("[data-diffnote-user-settings-save]")
        self.assertTrue(b.wait("!!document.querySelector('[data-diffnote-settings-page] .diffnote-error')"))
        # A blank name clears the configured one (falls back to git's, or the login name --
        # whichever it is here, it is not "別の人" any more, and `config get` says so too).
        b.set_value("[data-diffnote-user-setting-author]", "   ")
        b.click("[data-diffnote-user-settings-save]")
        self.assertTrue(b.wait_exists("[data-diffnote-user-settings-saved]"))
        b.click("[data-diffnote-settings-back]")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-author]').textContent!=='別の人'"))
        got = harness.diffnote("config", "get", "author").stdout
        self.assertIn("設定されていません", got, got)

    def test_quitting_without_saving_puts_the_review_back_as_it_was_when_the_server_started(self):
        self.serve()
        b = self.b
        before = show(self.review)
        card = self.card("mul の型")
        self.reply_to(card, "捨てる返信")
        self.assertIn("捨てる返信", show(self.review))
        self.assertFalse(b.exists("[data-diffnote-discard]"), "the way out is behind the arrow")
        b.click("[data-diffnote-quit-more]")
        self.assertTrue(b.wait_exists("[data-diffnote-discard]"))
        b.click("[data-diffnote-discard]")
        self.assertTrue(b.wait_exists("[data-diffnote-discard-confirm]"), "asked again")
        self.assertIn("捨てる返信", show(self.review), "nothing is thrown away before it is confirmed")
        b.click("[data-diffnote-discard-confirm]")
        self.assertTrue(b.wait("!document.getElementById('app')"))
        told = b.js("document.body.textContent")
        self.assertIn("保存せずに終了しました", told)
        self.assertIn("破棄しました", told)
        self.assertEqual(show(self.review), before)
        self.assertTrue(any("保存せずに終了" in l for l in self.wait_said()), "the terminal says so too")

    def wait_said(self):
        deadline = time.time() + 5
        while time.time() < deadline and not any("保存せずに終了" in l for l in self.server.said_more()):
            time.sleep(0.05)
        return self.server.said_more()

    def test_many_revisions_do_not_change_the_top_bar(self):
        self.serve()
        b = self.b
        before = b.js("document.querySelector('.diffnote-topbar').offsetHeight")
        # Many tabs, as a long review has.
        b.js("""(function(){var ul=document.querySelector('.diffnote-revisions ul'); var li=ul.querySelector('li');
          for (var i=0;i<12;i++) { var c=li.cloneNode(true); c.querySelector('a').textContent='#'+(i+3)+' abcdef'+i+' (2026-09-21)'; ul.appendChild(c); } })()""")
        time.sleep(0.2)
        self.assertEqual(b.js("document.querySelector('.diffnote-topbar').offsetHeight"), before, "the bar keeps its height")
        nav = b.js("(() => { const n = document.querySelector('.diffnote-revisions'); return {over: n.scrollWidth > n.clientWidth, bar: n.offsetHeight - n.clientHeight}; })()")
        self.assertTrue(nav["over"], "the tabs scroll sideways")
        self.assertEqual(nav["bar"], 0, "with no scroll bar taking height")
        for sel in ("[data-diffnote-shutdown]",):
            h = b.js(f"document.querySelector({json.dumps(sel)}).getBoundingClientRect().height")
            self.assertLess(h, 32, f"{sel} stays on one line")
        right = b.js("document.querySelector('[data-diffnote-quit-more]').getBoundingClientRect().right")
        self.assertLessEqual(right, b.js("innerWidth"), "and is in the window")
        # The wheel scrolls them sideways.
        b.js("document.querySelector('.diffnote-revisions').scrollLeft = 0")
        b.js("document.querySelector('.diffnote-revisions').dispatchEvent(new WheelEvent('wheel', {deltaY: 120, bubbles: true, cancelable: true}))")
        self.assertGreater(b.js("document.querySelector('.diffnote-revisions').scrollLeft"), 0)

    def test_the_author_is_at_the_foot_of_the_side_like_a_signed_in_user(self):
        self.serve()
        b = self.b
        pos = b.js("(() => { const u = document.querySelector('[data-diffnote-user]').getBoundingClientRect(); const s = document.querySelector('.diffnote-sidebar').getBoundingClientRect(); return {gap: s.bottom - u.bottom, inside: u.left >= s.left && u.right <= s.right}; })()")
        self.assertLess(pos["gap"], 12, "at the foot of the side")
        self.assertTrue(pos["inside"])
        self.assertEqual(b.text("[data-diffnote-user] [data-diffnote-author]"), "検証者")
        self.assertEqual(b.text(".diffnote-user__avatar"), "検")
        # Pressing it opens the settings screen, at ユーザー設定 (like the title
        # opens it at 全般).
        b.click("[data-diffnote-user-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-user-setting-author]"))
        time.sleep(0.1)  # let the screen's own Escape listener (a useEffect) attach
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))

    def test_each_blank_line_in_a_comment_is_kept(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, "上\n\n\n\n下")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        body = f"#{card} .diffnote-comment__body"
        self.assertTrue(b.wait(f"!document.querySelector('.is-pending') && [...document.querySelectorAll({json.dumps(body)})].some(e => e.textContent.includes('下'))"))
        # Three blank lines: the paragraph break, and two more, each one line high.
        self.assertEqual(b.js(f"[...document.querySelectorAll({json.dumps(body)})].find(e => e.textContent.includes('下')).querySelectorAll('.diffnote-blank').length"), 2)
        heights = b.js(f"(() => {{ const e = [...document.querySelectorAll({json.dumps(body)})].find(e => e.textContent.includes('下')); const p = e.querySelectorAll('p'); return [p[1].getBoundingClientRect().top - p[0].getBoundingClientRect().bottom, p[0].getBoundingClientRect().height]; }})()")
        self.assertGreater(heights[0], 2 * heights[1] * 0.9, "the gap is about two lines of text more than a single break")

    def test_a_line_break_in_a_comment_stays_one_without_a_blank_line(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, "一行目\n二行目\n\n別の段落")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        body = f"#{card} .diffnote-comment__body"
        # (Until it is saved, what is shown is the draft as it was typed.)
        self.assertTrue(b.wait(f"!document.querySelector('.is-pending') && [...document.querySelectorAll({json.dumps(body)})].some(e => e.textContent.includes('別の段落'))"))
        html = b.js(f"[...document.querySelectorAll({json.dumps(body)})].find(e => e.textContent.includes('別の段落')).innerHTML")
        self.assertIn("一行目<br>二行目", html)
        self.assertEqual(html.count("<p>"), 2, "a blank line is still a new paragraph")

    def test_the_shutdown_button_stops_the_server_and_says_roughly_what_was_saved(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "終了前の返信")
        b.js(f"document.getElementById({card!r}).querySelector('[data-diffnote-action]').click()")
        deadline = time.time() + 8
        while "解決" not in show(self.review) and time.time() < deadline:
            time.sleep(0.05)
        b.js("document.querySelector('[data-diffnote-shutdown]').click()")
        # (The page's own scripts are in its body, so look for the page going.)
        self.assertTrue(b.wait("!document.getElementById('app')"))
        told = b.js("document.body.textContent")
        self.assertIn("終了しました", told)
        self.assertIn("返信 1 件を追加", told)
        self.assertIn("解決 1 件", told)
        self.assertIn("保存先", told)
        self.assertIn("スレッド 4 件", told)
        self.assertIn(os.path.basename(self.review), told)


LOGIN = "table[data-diffnote-file='src/auth/login.ts']"


class NewThreadsOnLines(ServedCase):
    def gutter(self, kind, n, table=LOGIN):
        return f"{CUR} {table} tr[data-diffnote-{kind}='{n}'] .diffnote-line__gutter-{kind}"

    def send_box(self, text):
        self.write(".diffnote-composer-row textarea", text)
        self.b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")

    def test_pressing_a_line_number_opens_a_box_and_the_thread_is_put_in_place(self):
        self.serve(self.login)
        b = self.b
        before = b.count(f"{CUR} .diffnote-thread")
        b.click_at(self.gutter("new", 6))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:6")
        self.assertTrue(b.wait("document.activeElement.tagName==='TEXTAREA'"))
        self.send_box("1 行へのコメント")
        self.assertTrue(b.wait(f"!document.querySelector('.diffnote-composer-row') && document.querySelectorAll('{CUR} .diffnote-thread').length === {before + 1}"))
        self.assertTrue(self.same_page())
        row = self.gutter("new", 6)
        self.assertTrue(b.js(f"(function(){{var r=document.querySelector({row!r}).closest('tr'); var n=r.nextElementSibling; return r.classList.contains('diffnote-line--commented') && n.classList.contains('diffnote-thread-row') && n.textContent.includes('1 行へのコメント')}})()"))
        self.assertEqual(self.counts(), f"スレッド {before + 1} 件(解決済み 0 件)")
        self.assertTrue(any("login.ts:6" in l for l in show(self.review).splitlines() if "新規" in l))

    def test_the_box_for_chosen_lines_has_a_button_that_copies_a_link_to_them(self):
        self.serve(self.login)
        b = self.b
        b.drag(self.gutter("old", 8), self.gutter("new", 12))
        self.assertEqual(b.js("document.querySelector('.diffnote-compose__head .diffnote-copy').getAttribute('data-diffnote-copy')"),
                         "src/auth/login.ts:9-12@1")
        b.escape()
        # Removed lines only: the old side, with L.
        b.click_at(self.gutter("old", 8))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:L8")
        self.assertEqual(b.js("document.querySelector('.diffnote-compose__head .diffnote-copy').getAttribute('data-diffnote-copy')"),
                         "src/auth/login.ts:L8@1")

    def test_dragging_over_removed_and_added_lines_chooses_them_all(self):
        self.serve(self.login)
        b = self.b
        b.drag(self.gutter("old", 8), self.gutter("new", 12))
        self.assertEqual(b.count(".diffnote-select"), 5, "the removed line and four added ones")
        self.assertEqual(b.count(".diffnote-select-first"), 1)
        self.assertEqual(b.count(".diffnote-select-last"), 1)
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:9-12")
        self.send_box("ドラッグで選んだ範囲")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(any("login.ts:9-12 <- src/auth/login.ts:8" in l for l in show(self.review).splitlines()), show(self.review))

    def test_shift_click_extends_the_choice_and_a_draft_survives_choosing_again(self):
        self.serve(self.login)
        b = self.b
        b.click_at(self.gutter("new", 2))
        b.click_at(self.gutter("new", 4), modifiers=8)
        self.assertEqual(b.count(".diffnote-select"), 3)
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:2-4")
        self.write(".diffnote-composer-row textarea", "書きかけ")
        b.click_at(self.gutter("new", 5))
        self.assertEqual(b.count(".diffnote-composer-row"), 1)
        self.assertEqual(b.value(".diffnote-composer-row textarea"), "書きかけ")
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row') && !document.querySelector('.diffnote-select')"))

    def test_no_thread_is_highlighted_while_lines_are_being_chosen(self):
        self.serve(self.login)
        b = self.b
        b.hover(f"{CUR} .diffnote-thread summary")
        self.assertGreaterEqual(b.count(".diffnote-range"), 1)
        b.press(self.gutter("new", 6))
        self.assertEqual(b.count(".diffnote-range"), 0)
        for n in (5, 4, 3, 2):
            x, y = b.center(self.gutter("new", n))
            b.cdp.mouse("mouseMoved", x, y, 1)
            self.assertEqual(b.count(".diffnote-range"), 0, n)
            self.assertEqual(b.count(".diffnote-thread.diffnote-hover"), 0, n)
        b.release(self.gutter("new", 2))
        self.assertEqual(b.count(".diffnote-select-first"), 1)

    def test_a_failed_send_keeps_the_box_and_the_words(self):
        self.serve(self.login)
        b = self.b
        b.click_at(self.gutter("new", 6))
        self.write(".diffnote-composer-row textarea", "止まった後")
        self.server.shut_down(b)
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!!document.querySelector('.diffnote-compose .diffnote-error')"))
        self.assertEqual(b.value(".diffnote-composer-row textarea"), "止まった後")
        self.assertTrue(b.js("document.querySelector('.diffnote-compose').style.display !== 'none'"))

    def test_the_thread_is_followed_into_the_other_revision(self):
        self.serve(self.calc)
        b = self.b
        b.click_at(f"{CUR} table[data-diffnote-file='calc.py'] tr[data-diffnote-new='14'] .diffnote-line__gutter-new")
        self.send_box("mul の戻り値を確認")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        b.click("[data-diffnote-revision-link='0']")
        where = b.js("""(function(){var t=Array.from(document.querySelectorAll('#rev-0 .diffnote-thread')).find(function(t){return t.textContent.includes('mul の戻り値を確認')}); return t.querySelector('.diffnote-thread__where').textContent})()""")
        self.assertEqual(where, "calc.py:12", "line 14 of the new revision is line 12 of the old one")
        self.assertGreater(b.count("#rev-0 .diffnote-reply"), 0, "still interactive")


class IgnoreWhitespaceDefault(ServedCase):
    """The review says how the page starts; changing it on the page is for that page only."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.indent, _ = make_indent_review(cls.root)

    def test_switching_it_on_the_page_is_not_kept_in_the_review(self):
        self.serve(self.indent)
        b = self.b
        read = lambda: pathlib.Path(self.review).read_bytes()
        before = read()
        self.assertFalse(b.js("document.querySelector('[data-diffnote-ignore-space]').checked"), "the default is to show them")
        b.click("[data-diffnote-ignore-space]")
        self.assertTrue(b.wait("document.querySelectorAll('tr.diffnote-line--removed').length === 1"))
        time.sleep(0.3)
        self.assertEqual(read(), before, "nothing was written")
        self.assertEqual(b.js("fetch('/api/whitespace', {method: 'POST', headers: {'X-Diffnote': '1'}, body: '{}'}).then(r => r.status)"), 404, "and nothing to ask the server to keep")
        # A page opened again starts as the review says: not ignoring.
        b.reload(ready="!!document.querySelector('.diffnote-file')")
        self.assertFalse(b.js("document.querySelector('[data-diffnote-ignore-space]').checked"))
        self.assertEqual(b.count("tr.diffnote-line--removed"), 3)

    def test_a_review_that_says_to_ignore_it_starts_so_and_the_page_can_still_show_them(self):
        review = os.path.join(self.fresh("review"), "ws.diffnote")
        shutil.copy(self.indent, review)
        add_settings(review, {"ignore_whitespace": True})
        self.review = review
        self.server = Served(review, author="検証者")
        self.addCleanup(self.server.stop)
        b = self.b = self.browser
        b.open(self.server.url, ready="!!document.querySelector('.diffnote-file')")
        b.js("localStorage.setItem('diffnote-layout','unified')")
        b.reload(ready="!!document.querySelector('.diffnote-file')")
        self.assertTrue(b.js("document.querySelector('[data-diffnote-ignore-space]').checked"))
        self.assertEqual(b.count("tr.diffnote-line--removed"), 1)
        b.click("[data-diffnote-ignore-space]")
        self.assertTrue(b.wait("document.querySelectorAll('tr.diffnote-line--removed').length === 3"))
        self.assertIn("空白", show(review), "the setting is what `show` says")


class CompareWithAnEarlierRevision(ServedCase):
    """The base in the top bar is a menu: a revision can be looked at against an earlier one."""

    def choose(self, value):
        self.b.js("(s => { s.value = %r; s.dispatchEvent(new Event('change', {bubbles: true})); })(document.querySelector('[data-diffnote-base-select]'))" % value)

    def test_the_menu_offers_the_base_and_the_earlier_revisions_and_only_when_there_is_one(self):
        self.serve()
        b = self.b
        options = b.js("[...document.querySelectorAll('[data-diffnote-base-select] option')].map(o => o.textContent)")
        self.assertEqual(len(options), 2, options)
        self.assertRegex(options[0], r"^[0-9a-f]{7}$")
        self.assertTrue(options[1].startswith("#1 "), options)
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-base-select]')"), "nothing is earlier than the first")

    def test_the_revision_is_shown_against_the_chosen_one_and_nothing_is_recorded(self):
        self.serve()
        b = self.b
        before = entries(self.review)
        self.choose("0")
        self.assertTrue(b.wait_exists("[data-diffnote-compare-note]"))
        self.assertRegex(b.text("[data-diffnote-compare-note]"), r"^#1 [0-9a-f]{7} \.\. #2 [0-9a-f]{7}$")
        self.assertTrue(b.exists("[data-diffnote-compare-note] svg.diffnote-icon"), "marked as something to notice")
        self.assertIn("表示だけの切り替え", b.js("document.querySelector('[data-diffnote-compare-note]').title"), "the explanation is the tooltip")
        self.assertTrue(b.js("document.querySelector('[data-diffnote-base]').classList.contains('is-changed')"))
        # What changed from c2 to c3: a docstring (two lines) and one line replaced.
        self.assertTrue(b.wait(f"document.querySelectorAll('{CUR} tr.diffnote-line--added').length === 3"))
        self.assertEqual(b.count(f"{CUR} tr.diffnote-line--removed"), 1)
        # The threads are where they are in this view.
        self.card("mul の型")
        self.assertEqual(entries(self.review), before, "looking writes nothing")
        self.choose("")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-compare-note]')"))
        self.assertGreater(b.count(f"{CUR} tr.diffnote-line--added"), 3, "the base's diff again")

    def test_a_thread_on_a_line_is_written_against_the_base_whatever_it_is_compared_with(self):
        self.serve()
        b = self.b
        self.choose("0")
        self.assertTrue(b.wait_exists("[data-diffnote-compare-note]"))
        # A removed line is not in this revision: pressing it does nothing.
        b.click_at(f"{CUR} tr.diffnote-line--removed .diffnote-line__gutter-old")
        time.sleep(0.3)
        self.assertFalse(b.exists(".diffnote-composer-row"))
        # The `raise` line (line 9 of this revision) can be commented on.
        row = f"{CUR} tr.diffnote-line--added[data-diffnote-new='9'] .diffnote-line__gutter-new"
        b.click_at(row)
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "calc.py:9")
        self.write(".diffnote-composer-row textarea", "比べた画面で書きました")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(any("calc.py:9" in l for l in show(self.review).splitlines()), show(self.review))
        self.assertIn("比べた画面で書きました", show(self.review))
        # Back at the base: the thread is on the same line there.
        self.choose("")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-compare-note]')"))
        self.card("比べた画面で書きました")
        self.assertEqual(b.js("[...document.querySelectorAll('%s .diffnote-thread')].find(t => t.textContent.includes('比べた画面で書きました')).querySelector('.diffnote-thread__where').textContent" % CUR), "calc.py:9")

    def test_a_file_that_is_only_shown_for_its_thread_can_be_commented_on_too(self):
        repo = os.path.join(self.root, "onlythread")
        os.makedirs(repo)
        harness.git(repo, "init", "-q", "-b", "main")
        harness.write(repo, "a.txt", "a1\n")
        harness.write(repo, "b.txt", "b1\n")
        harness.git(repo, "add", "-A")
        harness.git(repo, "commit", "-q", "-m", "c1")
        harness.git(repo, "tag", "c1")
        harness.write(repo, "a.txt", "a2\n")
        harness.git(repo, "commit", "-q", "-am", "c2")
        harness.git(repo, "tag", "c2")
        master = os.path.join(self.root, "onlythread.diffnote")
        assert harness.diffnote("edit", "-f", master, "--base", "c1", "c2", cwd=repo, comments=[("+a2", "a への指摘")]).returncode == 0
        harness.write(repo, "b.txt", "b2\n")
        harness.git(repo, "commit", "-q", "-am", "c3")
        harness.git(repo, "tag", "c3")
        out = harness.diffnote("edit", "-f", master, "--base", "c1", "c3", cwd=repo, comments=[("GLOBAL", "二つ目")])
        assert out.returncode == 0, out.stdout + out.stderr
        self.serve(master)
        b = self.b
        self.choose("0")
        self.assertTrue(b.wait_exists("[data-diffnote-compare-note]"))
        section = f"{CUR} section.diffnote-file[data-diffnote-file='a.txt']"
        self.assertTrue(b.wait_exists(section), "a.txt is only here for its thread")
        self.assertEqual(b.count(f"{section} tr.diffnote-line--added"), 0, "it is not in what changed between the two")
        b.click(f"{section} [data-diffnote-add=file]")
        self.write(".diffnote-compose textarea", "a.txt 全体について")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-compose-wrap')"), b.js("[...document.querySelectorAll('.diffnote-error')].map(e=>e.textContent).join('|')"))
        self.assertTrue(any("ファイル全体: a.txt" in l for l in show(self.review).splitlines()), show(self.review))

    def test_a_file_marked_as_looked_at_stays_so_in_the_other_view(self):
        self.serve()
        b = self.b
        section = f"{CUR} section.diffnote-file[data-diffnote-file='calc.py']"
        b.click(f"{section} [data-diffnote-viewed]")
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(section)})"))
        self.choose("0")
        self.assertTrue(b.wait_exists("[data-diffnote-compare-note]"))
        self.assertFalse(b.exists(section), "still looked at")

    def test_it_falls_back_to_the_base_when_the_tab_changes_to_the_first_revision(self):
        self.serve()
        b = self.b
        self.choose("0")
        self.assertTrue(b.wait_exists("[data-diffnote-compare-note]"))
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-compare-note]')"))

    def test_the_server_refuses_what_is_not_an_earlier_revision(self):
        self.serve()
        b = self.b
        for query in ("rev=1&from=1", "rev=0&from=1", "rev=9&from=0", "rev=1"):
            status = b.js("fetch('/api/compare?%s').then(r => r.status)" % query)
            self.assertIn(status, (400,), query)


PNG_1X1 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
# The same picture with a byte after its end: a second attachment (they are
# told apart by their bytes), the same to look at.
PNG_1X1_TOO = base64.b64encode(base64.b64decode(PNG_1X1) + b"\0").decode()
SVG_OK = '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="blue"/></svg>'
SVG_BAD = '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)" width="8" height="8"></svg>'


class Images(ServedCase):
    def test_a_pasted_picture_is_put_in_the_review_and_shown_in_the_comment(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, "見てください")
        self.paste(box, "shot.png", "image/png", PNG_1X1, base64=True)
        self.assertTrue(b.wait("!!document.querySelector('[data-diffnote-attach-status]')"))
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-attach-status]').textContent.includes('画像を追加しました')"))
        self.assertIn("バンドルの大きさ", b.text("[data-diffnote-attach-status]"))
        text = b.js(f"document.querySelector({json.dumps(box)}).value")
        self.assertRegex(text, r"!\[画像\]\(diffnote-image:[0-9a-f]{64}\)")
        self.assertEqual(len([n for n in harness.zip_names(self.review) if n.startswith("images/")]), 1, "one image in the bundle")
        # Sent: the picture is in the comment, drawn as an <img>.
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        img = f"#{card} .diffnote-comment__body img.diffnote-image"
        self.assertTrue(b.wait_exists(img))
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(img)}).naturalWidth === 1"), "it loaded from the server")
        # Kept in the review as it is, and put in an export as it is.
        self.assertIn("diffnote-image:", show(self.review))
        exported = b.js("fetch('/export').then(r => r.text())")
        self.assertIn("data:image/png;base64,iVBORw0KGgo", exported)
        # ... and that page, opened from a file, shows it (nothing to ask).
        path = os.path.join(self.fresh("export"), "with-image.html")
        with open(path, "w", encoding="utf-8") as f:
            f.write(exported)
        b.open(pathlib.Path(path).as_uri(), ready="!!document.querySelector('.diffnote-comment__body img.diffnote-image')")
        self.assertTrue(b.wait("document.querySelector('.diffnote-comment__body img.diffnote-image').naturalWidth === 1"))

    def test_the_button_is_above_the_box_it_is_for(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        above = lambda box: b.js("(() => { const t = document.querySelector(%s).getBoundingClientRect(); const a = document.querySelector(%s).closest('form').querySelector('[data-diffnote-attach]').closest('label').getBoundingClientRect(); return a.bottom <= t.top + 1; })()" % (json.dumps(box), json.dumps(box)))
        self.assertTrue(above(f"#{card} .diffnote-reply textarea"), "reply")
        b.click(f"{CUR} [data-diffnote-add=global]")
        self.assertTrue(b.wait_exists(".diffnote-compose textarea[placeholder^='コメントを書く']"))
        self.assertTrue(above(".diffnote-compose textarea[placeholder^='コメントを書く']"), "new comment")

    def test_any_file_can_be_attached_and_is_offered_only_to_be_saved(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.paste(box, "notes.txt", "text/plain", "hello attachment")
        self.assertTrue(b.wait("!!document.querySelector('[data-diffnote-attach-status]') && document.querySelector('[data-diffnote-attach-status]').textContent.includes('を添付しました')"))
        self.assertIn("「notes.txt」", b.text("[data-diffnote-attach-status]"))
        self.assertRegex(b.js(f"document.querySelector({json.dumps(box)}).value"), r"\[notes\.txt\]\(diffnote-file:[0-9a-f]{64}\)")
        self.assertEqual(len([n for n in harness.zip_names(self.review) if n.startswith("attachments/")]), 1)
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        link = f"#{card} .diffnote-comment__body a.diffnote-attachment"
        self.assertTrue(b.wait_exists(link))
        self.assertEqual(b.js(f"document.querySelector({json.dumps(link)}).getAttribute('download')"), "notes.txt")
        href = b.js(f"document.querySelector({json.dumps(link)}).getAttribute('href')")
        self.assertTrue(href.startswith("/api/attachments/"), href)
        got = b.js("fetch(%s).then(async r => [r.headers.get('content-disposition'), r.headers.get('content-type'), await r.text()])" % json.dumps(href))
        self.assertTrue(got[0].startswith("attachment;"), got)
        self.assertEqual(got[1], "application/octet-stream")
        self.assertEqual(got[2], "hello attachment")
        # The export has it inside, still to be saved.
        exported = b.js("fetch('/export').then(r => r.text())")
        self.assertIn("data:application/octet-stream;base64,", exported)
        path = os.path.join(self.fresh("export"), "with-file.html")
        with open(path, "w", encoding="utf-8") as f:
            f.write(exported)
        b.open(pathlib.Path(path).as_uri(), ready="!!document.querySelector('a.diffnote-attachment')")
        self.assertTrue(b.js("document.querySelector('a.diffnote-attachment').getAttribute('href').startsWith('data:application/octet-stream;base64,')"))
        self.assertEqual(b.js("document.querySelector('a.diffnote-attachment').getAttribute('download')"), "notes.txt")

    def test_a_file_dropped_on_a_box_is_attached_and_the_box_says_it_can_take_it(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        js = """(function(sel, type){
          var dt = new DataTransfer(); dt.items.add(new File([new TextEncoder().encode('dropped data')], 'dropped.txt', {type: 'text/plain'}));
          var ta = document.querySelector(sel);
          var ev = new DragEvent(type, {dataTransfer: dt, bubbles: true, cancelable: true});
          ta.dispatchEvent(ev);
          return [ev.defaultPrevented, ta.classList.contains('is-dropping')];
        })(%s, %s)"""
        over = b.js(js % (json.dumps(box), json.dumps("dragover")))
        self.assertEqual(over, [True, True], "the browser is told it may drop here, and the box shows it")
        b.js(js % (json.dumps(box), json.dumps("dragleave")))
        self.assertFalse(b.js(f"document.querySelector({json.dumps(box)}).classList.contains('is-dropping')"))
        dropped = b.js(js % (json.dumps(box), json.dumps("drop")))
        self.assertEqual(dropped[0], True, "the browser does not open the file itself")
        self.assertTrue(b.wait("!!document.querySelector('[data-diffnote-attach-status]') && document.querySelector('[data-diffnote-attach-status]').textContent.includes('「dropped.txt」を添付しました')"))
        self.assertRegex(b.js(f"document.querySelector({json.dumps(box)}).value"), r"\[dropped\.txt\]\(diffnote-file:[0-9a-f]{64}\)")

    def test_a_file_over_the_reviews_limit_is_told_so_and_not_sent(self):
        self.serve()
        b = self.b
        self.assertEqual(b.js("JSON.parse(document.getElementById('diffnote-data').textContent).attachment_limit"), 5 * 1024 * 1024)
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        b.js("""(function(sel){
          var dt = new DataTransfer(); dt.items.add(new File([new Uint8Array(6 * 1024 * 1024)], 'big.bin', {type: 'application/octet-stream'}));
          var ta = document.querySelector(sel); ta.focus();
          ta.dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true}));
        })(%s)""" % json.dumps(box))
        self.assertTrue(b.wait("!!document.querySelector('.is-failed[data-diffnote-attach-status]')"))
        said = b.text("[data-diffnote-attach-status]")
        self.assertIn("big.bin", said)
        self.assertIn("超えています", said)
        self.assertEqual([n for n in harness.zip_names(self.review) if n.startswith("attachments/")], [], "nothing was sent")

    def test_an_svg_is_taken_only_if_nothing_in_it_runs(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.paste(box, "bad.svg", "image/svg+xml", SVG_BAD)
        self.assertTrue(b.wait("!!document.querySelector('.is-failed[data-diffnote-attach-status]')"))
        self.assertIn("SVG", b.text("[data-diffnote-attach-status]"))
        self.assertNotIn("diffnote-image:", b.js(f"document.querySelector({json.dumps(box)}).value"))
        self.paste(box, "ok.svg", "image/svg+xml", SVG_OK)
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-attach-status]').textContent.includes('画像を追加しました')"))
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        img = f"#{card} .diffnote-comment__body img.diffnote-image"
        # Asked in two parts, so that a failure says which: the comment showing
        # the picture at all, or the picture itself arriving.
        self.assertTrue(b.wait_exists(img), "the comment shows the picture")
        if not b.wait(f"document.querySelector({json.dumps(img)}).naturalWidth === 8"):
            # This one has failed about one full run in four, always stuck
            # rather than slow, and never on its own. Whatever it is, the next
            # time it happens it can say so: what the picture's address is, how
            # far it got, and what the server answers for it now.
            print("\n-- picture:", b.js(
                "(function(i){return i ? JSON.stringify({src: i.getAttribute('src'), complete: i.complete,"
                " width: i.naturalWidth, alt: i.alt}) : 'no img'})(document.querySelector(%s))" % json.dumps(img)),
                file=sys.stderr)
            print("-- server says:", b.js(
                "fetch(document.querySelector(%s).getAttribute('src'), {cache: 'reload'})"
                ".then(function (r) { return r.status + ' ' + r.headers.get('content-type'); })"
                ".catch(function (e) { return 'unreachable: ' + e; })" % json.dumps(img)), file=sys.stderr)
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(img)}).naturalWidth === 8"),
                        "the picture is the one that was sent")

    def test_a_link_to_an_image_elsewhere_is_only_its_text(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, "![外の画像](https://example.invalid/a.png) と ![](javascript:alert(1))")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).textContent.includes('外の画像')"))
        self.assertEqual(b.count(f"#{card} img"), 0, "nothing is loaded from an address")


class ImageZoom(ServedCase):
    """An image in a comment, shown by itself over the page."""

    def comment_with_a_picture(self, width, height):
        """Writes a comment showing a picture of that size, and gives back the
        card it is in."""
        b = self.b
        card = self.card("mul の型")
        ident = b.js("""(async function () {
          var c = document.createElement('canvas'); c.width = %d; c.height = %d;
          var g = c.getContext('2d'); g.fillStyle = '#1f77b4'; g.fillRect(0, 0, c.width, c.height);
          var blob = await new Promise(function (r) { c.toBlob(r, 'image/png'); });
          return (await Diffnote.api.upload(blob)).id;
        })()""" % (width, height))
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, f"![再現時の画面](diffnote-image:{ident})")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('img.diffnote-image')"))
        return card

    def test_a_picture_opens_by_itself_at_its_own_size_and_escape_closes_it(self):
        self.serve()
        b = self.b
        card = self.comment_with_a_picture(700, 300)
        thumb = f"#{card} img.diffnote-image"
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(thumb)}).complete"))
        b.click(thumb)
        self.assertTrue(b.wait_exists("[data-diffnote-zoom]"))
        self.assertEqual(
            b.js("document.querySelector('[data-diffnote-zoom-image]').src"),
            b.js(f"document.querySelector({json.dumps(thumb)}).src"),
            "the same picture")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-zoom-image]').complete"))
        self.assertEqual(
            b.js("(function(){var i=document.querySelector('[data-diffnote-zoom-image]'); return [i.clientWidth, i.naturalWidth]})()"),
            [700, 700], "at its own size, not the comment's width")
        self.assertEqual(b.js("document.body.style.overflow"), "hidden", "the page behind doesn't scroll")
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-zoom]')"))
        self.assertEqual(b.js("document.body.style.overflow"), "", "and scrolls again after")

    def test_one_too_big_for_the_window_can_be_fitted_to_it_and_back(self):
        self.serve()
        b = self.b
        card = self.comment_with_a_picture(2400, 1800)
        b.click(f"#{card} img.diffnote-image")
        self.assertTrue(b.wait_exists("[data-diffnote-zoom-image]"))
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-zoom-image]').complete"))
        self.assertEqual(b.js("document.querySelector('[data-diffnote-zoom-image]').clientWidth"), 2400)
        b.click("[data-diffnote-zoom-image]")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-zoom]').classList.contains('is-fitted')"))
        self.assertLess(b.js("document.querySelector('[data-diffnote-zoom-image]').clientWidth"), 2400)
        self.assertTrue(b.exists("[data-diffnote-zoom]"), "pressing it fits it, it doesn't close")
        b.click("[data-diffnote-zoom-image]")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-zoom-image]').clientWidth === 2400"))

    def test_it_works_in_an_exported_page_opened_from_a_file(self):
        self.serve()
        b = self.b
        self.comment_with_a_picture(500, 250)
        exported = b.js("fetch('/export').then(r => r.text())")
        path = os.path.join(self.fresh("export"), "with-picture.html")
        with open(path, "w", encoding="utf-8") as f:
            f.write(exported)
        b.open(pathlib.Path(path).as_uri(), ready="!!document.querySelector('img.diffnote-image')")
        self.assertTrue(b.js("document.querySelector('img.diffnote-image').src.startsWith('data:image/png')"),
                        "the picture is in the page, not at an address")
        self.assertTrue(b.wait("document.querySelector('img.diffnote-image').complete"))
        b.click("img.diffnote-image")
        self.assertTrue(b.wait_exists("[data-diffnote-zoom]"))
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-zoom-image]').complete"))
        self.assertEqual(
            b.js("(function(){var i=document.querySelector('[data-diffnote-zoom-image]'); return [i.clientWidth, i.naturalWidth]})()"),
            [500, 500], "at its own size, with no server to ask")
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-zoom]')"))

    def test_pressing_one_that_fits_the_window_closes_it(self):
        self.serve()
        b = self.b
        card = self.comment_with_a_picture(700, 300)
        b.click(f"#{card} img.diffnote-image")
        self.assertTrue(b.wait_exists("[data-diffnote-zoom-image]"))
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-zoom-image]').complete"))
        self.assertEqual(b.js("getComputedStyle(document.querySelector('[data-diffnote-zoom-image]')).cursor"), "zoom-out")
        b.click("[data-diffnote-zoom-image]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-zoom]')"), "the cursor said so")

    def test_the_close_button_and_the_space_around_it_close_it(self):
        self.serve()
        b = self.b
        card = self.comment_with_a_picture(400, 200)
        b.click(f"#{card} img.diffnote-image")
        self.assertTrue(b.wait_exists("[data-diffnote-zoom-close]"))
        b.click("[data-diffnote-zoom-close]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-zoom]')"))
        b.click(f"#{card} img.diffnote-image")
        self.assertTrue(b.wait_exists("[data-diffnote-zoom]"))
        b.click("[data-diffnote-zoom]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-zoom]')"), "the space around it closes it too")


class Timeline(ServedCase):
    """タイムライン: the review's own log, read in order."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        repo = os.path.join(cls.root, "tl-repo")
        os.makedirs(repo)
        harness.git(repo, "init", "-q", "-b", "main")
        body = "".join("line %d\n" % n for n in range(1, 13))
        harness.write(repo, "calc.py", body)
        harness.git(repo, "add", "-A")
        harness.git(repo, "commit", "-q", "-m", "はじめのコミット")
        harness.git(repo, "tag", "c1")
        harness.write(repo, "calc.py", body.replace("line 7\n", "line seven\n"))
        harness.git(repo, "commit", "-q", "-am",
                    "7 行目を書き直した\n\n数字のままだと読みづらかったため。\n\nCo-Authored-By: 誰か <x@example.com>")
        harness.git(repo, "tag", "c2")
        cls.timeline_review = os.path.join(cls.root, "tl.diffnote")
        harness.set_user_author("共田")
        assert harness.diffnote("edit", "-f", cls.timeline_review, "--base", "c1", "c2", cwd=repo, comments=[
            ("GLOBAL", "全体としてよさそうです"),
            ("+line seven", "ここは数字のままでもよいのでは。"),
            ("@raw:+line seven", ">!resolve"),
        ]).returncode == 0

    def open_timeline(self):
        b = self.b
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page]"))
        b.click("[data-diffnote-settings-nav='timeline']")
        self.assertTrue(b.wait_exists("[data-diffnote-timeline-pane]"))

    def test_what_happened_is_listed_in_order_with_the_commits_each_revision_brought(self):
        self.serve(self.timeline_review)
        b = self.b
        self.open_timeline()
        kinds = b.js("[...document.querySelectorAll('[data-diffnote-timeline]')].map(e => e.dataset.diffnoteTimeline)")
        self.assertEqual(kinds[:2], ["started", "revision"], "oldest first: the review, then what it was made of")
        self.assertIn("comments", kinds, "a run of comments by one person is one line")
        self.assertIn("resolved", kinds)
        # The commits of the revision, with what they said folded away.
        self.assertEqual(b.count("[data-diffnote-commit]"), 1, "the one commit c1..c2 brought")
        row = "[data-diffnote-commit]"
        self.assertIn("7 行目を書き直した", b.text(row))
        self.assertFalse(b.js("document.querySelector(%s).open" % json.dumps(row + " details")),
                         "the message is folded until it is asked for")
        b.js("document.querySelector(%s).open = true" % json.dumps(row + " details"))
        self.assertTrue(b.wait("!!document.querySelector('%s .diffnote-timeline__body')" % row))
        self.assertIn("数字のままだと", b.text(row + " .diffnote-timeline__body"))
        self.assertNotIn("Co-Authored-By", b.text(row + " .diffnote-timeline__body"),
                         "the trailers are apart from what was written")
        self.assertIn("Co-Authored-By", b.text(row + " .diffnote-timeline__trailers"))
        self.assertIn("calc.py", b.text(row + " [data-diffnote-commit-files]"))

    def test_an_entry_leads_to_what_it_is_about(self):
        self.serve(self.timeline_review)
        b = self.b
        self.open_timeline()
        # A run of comments opens onto the comments in it.
        b.js("document.querySelector('[data-diffnote-timeline-run]').open = true")
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-timeline-thread]').length > 1"))
        b.click("[data-diffnote-timeline-thread]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"), "the screen closes")
        self.assertTrue(b.wait("location.hash.includes('thread')"), "and the address says where it went")
        # And a revision entry goes to that revision's tab.
        self.open_timeline()
        b.click("[data-diffnote-timeline-revision]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))
        self.assertTrue(b.wait("!!document.querySelector('%s')" % CUR))


class AttachmentsScreen(ServedCase):
    """The 添付 section of the settings screen: what the comments have
    attached, what uses it, and taking one out."""

    def attach(self, kind, name, content, mime=None, base64=False):
        """Attaches through the page's own calls (what pasting ends up doing),
        and gives back the id it is stored under."""
        made = "new Blob([bytes], {type: %s})" % json.dumps(mime or "application/octet-stream")
        call = ("Diffnote.api.upload(%s)" % made) if kind == "image" else (
            "Diffnote.api.uploadFile(%s, %s)" % (made, json.dumps(name)))
        return self.b.js("""(function () {
          var bytes = %s ? Uint8Array.from(atob(%s), function (c) { return c.charCodeAt(0); })
                         : new TextEncoder().encode(%s);
          return %s.then(function (r) { return r.id; });
        })()""" % ("true" if base64 else "false", json.dumps(content),
                   json.dumps(content), call))

    def open_attachments(self, rows):
        """Opens the 添付 section and waits for the `rows` it should list (the
        screen asks for a fresh model as it opens: an upload doesn't bring
        one)."""
        b = self.b
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page]"))
        b.click("[data-diffnote-settings-nav='attachments']")
        self.assertTrue(b.wait_exists("[data-diffnote-attachments-pane]"))
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-attached]').length === %d" % rows))

    def stored_images(self):
        return [n for n in harness.zip_names(self.review) if n.startswith("images/")]

    def rows(self):
        return self.b.js("""[...document.querySelectorAll('[data-diffnote-attached]')].map(function (li) {
          return { id: li.dataset.diffnoteAttached, text: li.textContent,
                   unused: !!li.querySelector('[data-diffnote-attached-unused]'),
                   uses: [...li.querySelectorAll('[data-diffnote-attached-use]')].map(function (u) { return u.textContent; }) };
        })""")

    def test_everything_attached_is_listed_with_what_uses_it_unused_first(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        shown = self.attach("image", "shot.png", PNG_1X1, "image/png", base64=True)
        saved = self.attach("file", "ログ.zip", "PK an archive", "application/zip")
        spare = self.attach("image", "spare.svg", SVG_OK, "image/svg+xml")
        self.write(box, f"見てください ![再現時の画面](diffnote-image:{shown}) と [ログ.zip](diffnote-file:{saved})")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('img.diffnote-image')"))
        self.open_attachments(3)
        rows = self.rows()
        self.assertEqual([r["id"] for r in rows][0], spare, "the one nothing uses comes first")
        self.assertTrue(rows[0]["unused"])
        self.assertIn("未使用", rows[0]["text"])
        self.assertIn("(名前なし)", rows[0]["text"], "no comment names it")
        self.assertFalse(rows[1]["unused"] or rows[2]["unused"])
        by_id = {r["id"]: r for r in rows}
        self.assertIn("再現時の画面", by_id[shown]["text"], "an image is called what the comment calls it")
        self.assertIn("image/png", by_id[shown]["text"])
        self.assertIn("ログ.zip", by_id[saved]["text"])
        self.assertEqual(len(by_id[shown]["uses"]), 1)
        self.assertRegex(by_id[shown]["uses"][0], r"^\S+:\d", "a use says where that comment is")
        # An image is shown as itself; a download is offered for each.
        self.assertTrue(b.wait(f"document.querySelector('[data-diffnote-attached=\"{shown}\"] img').naturalWidth === 1"))
        self.assertEqual(b.count("[data-diffnote-attached-download]"), 3)
        self.assertEqual(
            b.js(f"document.querySelector('[data-diffnote-attached-download=\"{saved}\"]').getAttribute('download')"),
            "ログ.zip", "a file keeps the name the comment gives it")

    def test_an_image_goes_by_the_name_of_the_file_it_was_attached_from(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        # One attached from a file, and one out of the clipboard -- which
        # Chrome calls `image.png` whatever it is a picture of, so the review
        # is better off knowing nothing about that one.
        self.paste(box, "図 1.png", "image/png", PNG_1X1, base64=True)
        self.assertTrue(b.wait("!!document.querySelector('[data-diffnote-attach-status]:not(.is-failed)')"))
        self.paste(box, "image.png", "image/png", PNG_1X1_TOO, base64=True)
        self.assertTrue(b.wait(f"(document.querySelector({json.dumps(box)}).value.match(/diffnote-image:/g) || []).length === 2"))
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        self.assertTrue(b.wait(f"document.getElementById({card!r}).querySelectorAll('img.diffnote-image').length === 2"))

        self.open_attachments(2)
        by_id = {r["id"]: r for r in self.rows()}
        named = b.js("""[...document.querySelectorAll('[data-diffnote-attached]')].map(function (li) {
          return [li.dataset.diffnoteAttached, li.querySelector('[data-diffnote-attached-download]').getAttribute('download')];
        })""")
        from_file = [id for id, name in named if name == "図 1.png"]
        self.assertEqual(len(from_file), 1, f"it is saved under its own name: {named}")
        self.assertIn("図 1.png", by_id[from_file[0]]["text"], "and listed under it")
        pasted = [id for id, name in named if id != from_file[0]]
        self.assertRegex(dict(named)[pasted[0]], r"^diffnote-[0-9a-f]{12}\.png$",
                         "the pasted one never had a name worth keeping")
        self.assertIn("画像", by_id[pasted[0]]["text"], "so it goes by what the comment calls it")

    def test_an_image_can_be_looked_at_by_itself(self):
        # The thumbnail is 48px: the only way to tell what it is is to open it.
        self.serve()
        b = self.b
        shown = self.attach("image", "shot.png", PNG_1X1, "image/png", base64=True)
        self.open_attachments(1)
        b.click(f"[data-diffnote-attached-zoom='{shown}']")
        self.assertTrue(b.wait_exists("[data-diffnote-zoom-image]"))
        self.assertEqual(
            b.js("document.querySelector('[data-diffnote-zoom-image]').src"),
            b.js(f"document.querySelector('[data-diffnote-attached=\"{shown}\"] img').src"),
            "the same picture the row shows")
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-zoom]')"))
        self.assertTrue(b.exists("[data-diffnote-attachments-pane]"), "and the screen is still there")

    def test_a_use_goes_to_the_comment_that_shows_it(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        shown = self.attach("image", "shot.png", PNG_1X1, "image/png", base64=True)
        self.write(box, f"![図](diffnote-image:{shown})")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('img.diffnote-image')"))
        self.open_attachments(1)
        where = b.text("[data-diffnote-attached-use]")
        self.assertTrue(where, "the comment's place is the link")
        b.click("[data-diffnote-attached-use]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"), "the screen closes")
        self.assertTrue(b.wait(f"location.hash.includes('thread')"), "and the address says where it went")

    def test_an_attachment_is_deleted_after_confirming_and_the_comment_is_left_as_written(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        shown = self.attach("image", "shot.png", PNG_1X1, "image/png", base64=True)
        self.write(box, f"![図](diffnote-image:{shown})")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('img.diffnote-image')"))
        self.open_attachments(1)
        # Asked about first, and giving up leaves it alone.
        b.click(f"[data-diffnote-attached-delete='{shown}']")
        self.assertTrue(b.wait_exists("[data-diffnote-attached-warn]"))
        self.assertIn("リンクは切れた", b.text("[data-diffnote-attached-warn]"), "it says what deleting does")
        b.js("document.querySelector('[data-diffnote-attached-warn] .diffnote-button:not([data-diffnote-attached-delete-ok])').click()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-attached-warn]')"))
        self.assertEqual(len(self.stored_images()), 1)
        # Confirmed: gone from the bundle and from the list, the comment as it was.
        b.click(f"[data-diffnote-attached-delete='{shown}']")
        b.click("[data-diffnote-attached-delete-ok]")
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-attached]').length === 0"))
        self.assertEqual(self.stored_images(), [])
        self.assertIn(f"diffnote-image:{shown}", show(self.review), "what was written stays written")
        # The comment still says what it said; the image it named is just gone.
        b.click("[data-diffnote-settings-back]")
        self.assertTrue(b.wait(f"!!document.getElementById({card!r}).querySelector('img.diffnote-image')"))
        self.assertEqual(
            b.js(f"fetch(document.getElementById({card!r}).querySelector('img.diffnote-image').src, {{cache: 'reload'}}).then(r => r.status)"),
            404, "and the server no longer has it")


class Quoting(ServedCase):
    """Quoting a comment (or what was chosen in it) in the reply box of its thread."""

    def select(self, card, start, end):
        """Chooses characters of the first paragraph of the first comment, as a mouse would."""
        return self.b.js("""(function(id, start, end){
          var body = document.getElementById(id).querySelector('.diffnote-comment__body');
          var node = body.querySelector('p').firstChild;
          var r = document.createRange(); r.setStart(node, start); r.setEnd(node, end);
          var sel = window.getSelection(); sel.removeAllRanges(); sel.addRange(r);
          body.dispatchEvent(new MouseEvent('mouseup', {bubbles: true}));
          return node.data.slice(start, end);
        })(%s, %d, %d)""" % (json.dumps(card), start, end))

    def test_what_is_chosen_in_a_comment_is_offered_as_a_quotation_for_a_reply(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.assertFalse(b.exists("[data-diffnote-quote-selection]"))
        chosen = self.select(card, 5, 9)
        self.assertEqual(chosen, "型を確認")
        self.assertTrue(b.wait_exists("[data-diffnote-quote-selection]"), "offered next to what was chosen")
        b.click("[data-diffnote-quote-selection]")
        self.assertEqual(b.value(box), "> 型を確認\n\n")
        self.assertTrue(b.js(f"document.activeElement === document.querySelector({json.dumps(box)})"), "ready to be written under")
        self.assertFalse(b.exists("[data-diffnote-quote-selection]"), "and the offer is gone")
        # Another goes after what is there.
        self.write(box, "> 型を確認\n\nはい。\n")
        self.select(card, 0, 3)
        self.assertTrue(b.wait_exists("[data-diffnote-quote-selection]"))
        b.click("[data-diffnote-quote-selection]")
        self.assertEqual(b.value(box), "> 型を確認\n\nはい。\n\n> mul\n\n")
        # Nothing chosen, nothing offered.
        b.js("window.getSelection().removeAllRanges()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-quote-selection]')"))

    def test_the_whole_comment_can_be_quoted_from_its_menu_as_it_was_written(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        b.click(f"#{card} [data-diffnote-comment]:not([data-diffnote-mine]) [data-diffnote-quote]")
        self.assertEqual(b.value(box), "> mul の型を確認してください。\n\n")
        # Sent, it is a quotation in the thread.
        self.write(box, b.value(box) + "確認しました。")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        quote = f"#{card} .diffnote-comment__body blockquote"
        self.assertTrue(b.wait_exists(quote))
        self.assertIn("mul の型を確認してください。", b.text(quote))

    def test_a_quotation_is_a_bar_and_quieter_text_not_only_an_indent(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, "> 引用されたことば\n\n本文")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        quote = f"#{card} .diffnote-comment__body blockquote"
        self.assertTrue(b.wait_exists(quote))
        style = b.js("(() => { const q = getComputedStyle(document.querySelector(%s)); const p = getComputedStyle(document.querySelector(%s).closest('.diffnote-comment__body')); return {bar: q.borderLeftWidth, style: q.borderLeftStyle, pad: q.paddingLeft, color: q.color, body: p.color}; })()" % (json.dumps(quote), json.dumps(quote)))
        self.assertEqual(style["bar"], "4px")
        self.assertEqual(style["style"], "solid")
        self.assertEqual(style["pad"], "14px")
        self.assertNotEqual(style["color"], style["body"], "quieter than the text under it")


class BoxesGrowWithWhatIsWritten(ServedCase):
    def height(self, selector):
        return self.b.js("document.querySelector(%s).offsetHeight" % json.dumps(selector))

    def test_a_box_grows_and_shrinks_with_the_lines_and_stops_at_half_the_window(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        start = self.height(box)
        self.write(box, "\n".join(f"行 {n}" for n in range(1, 9)))
        grown = self.height(box)
        self.assertGreater(grown, start + 80, "eight lines are shown, not two and a scroll bar")
        self.assertFalse(b.js("(e => e.scrollHeight > e.clientHeight + 1)(document.querySelector(%s))" % json.dumps(box)), "all of it is in view")
        # Fewer lines: smaller again, to what it started as.
        self.write(box, "一行だけ")
        self.assertLessEqual(self.height(box), start + 2)
        # A wrapped line counts as lines too.
        self.write(box, "長い行 " * 300)
        self.assertGreater(self.height(box), start + 40)
        # Very many: it stops at half the window, and scrolls.
        self.write(box, "\n".join(f"行 {n}" for n in range(1, 200)))
        limit = b.js("innerHeight * 0.5")
        self.assertLessEqual(self.height(box), limit + 2)
        self.assertTrue(b.js("(e => e.scrollHeight > e.clientHeight + 1)(document.querySelector(%s))" % json.dumps(box)))

    def test_the_box_for_a_new_comment_and_for_editing_grow_too(self):
        self.serve()
        b = self.b
        b.click(f"{CUR} [data-diffnote-add=global]")
        box = ".diffnote-compose textarea[placeholder^='コメントを書く']"
        self.assertTrue(b.wait_exists(box))
        start = self.height(box)
        self.write(box, "\n".join(["あ"] * 7))
        self.assertGreater(self.height(box), start + 60)
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-compose-wrap')"))
        # Editing what was written (seven lines): opens as tall as they are.
        b.click("[data-diffnote-global] [data-diffnote-mine] [data-diffnote-edit]")
        edit = "[data-diffnote-edit-form] textarea"
        self.assertTrue(b.wait_exists(edit))
        self.assertGreater(self.height(edit), start + 60)
        self.assertFalse(b.js("(e => e.scrollHeight > e.clientHeight + 1)(document.querySelector(%s))" % json.dumps(edit)))


class EmojiTable(ServedCase):
    def test_the_table_hangs_over_the_page_and_the_file_under_it_does_not_move(self):
        # A file's diff scrolls sideways, which makes it a box that clips
        # both ways: a table opened inside it would be cut off, would give
        # that file a scrollbar, and taking the cursor to the search box
        # would scroll the file out from under the reader.
        self.serve()
        b = self.b
        card = self.card("mul の型")
        form = f"#{card} .diffnote-reply"
        scroller = f"#{card}"
        b.js(f"document.querySelector({json.dumps(form)}).scrollIntoView({{block: 'end'}})")
        time.sleep(0.2)
        room = lambda: b.js("(() => { const s = document.querySelector('%s').closest('.diffnote-diff-scroll');"
                            " return s ? [s.scrollHeight - s.clientHeight, s.scrollTop] : null; })()" % scroller)
        where = lambda: b.js("[window.scrollY, window.scrollX]")
        before, page = room(), where()
        b.click(f"{form} [data-diffnote-emoji-button]")
        self.assertTrue(b.wait_exists("[data-diffnote-emoji-panel]"))
        self.assertTrue(b.wait("document.activeElement === document.querySelector('[data-diffnote-emoji-search]')"))
        self.assertEqual(where(), page, "nothing was scrolled to show it")
        if before is not None:
            self.assertEqual(room(), before, "and the file it is over gained no scrolling of its own")
        # It is over the page, not inside the file, and all of it is in view.
        fits = b.js("""(() => {
          const p = document.querySelector('[data-diffnote-emoji-panel]').getBoundingClientRect();
          return [getComputedStyle(document.querySelector('[data-diffnote-emoji-panel]')).position,
                  p.top >= 0 && p.bottom <= innerHeight, p.left >= 0 && p.right <= innerWidth];
        })()""")
        self.assertEqual(fits, ["fixed", True, True], fits)

    def test_an_emoji_is_chosen_from_a_table_and_put_where_the_cursor_is(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        form = f"#{card} .diffnote-reply"
        box = f"{form} textarea"
        self.write(box, "前後")
        b.js(f"(e => {{ e.focus(); e.setSelectionRange(1, 1); }})(document.querySelector({json.dumps(box)}))")
        b.click(f"{form} [data-diffnote-emoji-button]")
        self.assertTrue(b.wait_exists("[data-diffnote-emoji-panel]"))
        self.assertTrue(b.wait("document.activeElement === document.querySelector('[data-diffnote-emoji-search]')"), "ready to search")
        # All of them at first, the reaction ones first; a word narrows them.
        total = b.count("[data-diffnote-emoji]")
        self.assertGreaterEqual(total, 100)
        self.assertEqual(b.js("[...document.querySelectorAll('[data-diffnote-emoji]')].slice(0, 8).map(e => e.textContent).join('')"), "👍👎😄🎉😕❤️🚀👀")
        self.write_search("バグ")
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-emoji]').length < 6"))
        b.click("[data-diffnote-emoji=bug]")
        self.assertFalse(b.exists("[data-diffnote-emoji-panel]"), "the table is shut")
        self.assertEqual(b.value(box), "前🐛後")
        self.assertTrue(b.js(f"document.activeElement === document.querySelector({json.dumps(box)})"), "back in the box")
        self.assertEqual(b.js(f"document.querySelector({json.dumps(box)}).selectionStart"), 3, "after the emoji (a UTF-16 count: 前 is one, 🐛 two)")

    def test_enter_takes_the_first_found_and_escape_shuts_the_table_and_a_miss_is_told(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        form = f"#{card} .diffnote-reply"
        box = f"{form} textarea"
        b.click(f"{form} [data-diffnote-emoji-button]")
        self.assertTrue(b.wait_exists("[data-diffnote-emoji-search]"))
        self.write_search("zzzz")
        self.assertTrue(b.wait_exists(".diffnote-emoji__none"))
        self.write_search(":tada:")
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-emoji]').length === 1"))
        b.js("document.querySelector('[data-diffnote-emoji-search]').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true, cancelable: true}))")
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(box)}).value === '🎉'"))
        b.click(f"{form} [data-diffnote-emoji-button]")
        self.assertTrue(b.wait_exists("[data-diffnote-emoji-panel]"))
        time.sleep(0.2)  # (the table listens for Escape once it has been drawn)
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-emoji-panel]')"))
        self.assertEqual(b.value(box), "🎉", "nothing was added")

    def test_a_shortcode_is_shown_as_its_emoji_and_the_text_stays_as_written(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        box = f"#{card} .diffnote-reply textarea"
        self.write(box, "承認 :+1: :tada: 時刻 12:30:45 :nope: `:bug:`")
        b.js(f"document.querySelector({json.dumps(box)}).form.requestSubmit()")
        body = f"#{card} [data-diffnote-mine] .diffnote-comment__body"
        self.assertTrue(b.wait(f"!document.querySelector('.is-pending') && !!document.querySelector({json.dumps(body)})"))
        self.assertEqual(b.text(body), "承認 👍 🎉 時刻 12:30:45 :nope: :bug:", "code is left as it is")
        self.assertIn(":+1: :tada:", show(self.review), "what was written is what is kept")
        # The same in a page that only shows the review.
        exported = b.js("fetch('/export').then(r => r.text())")
        path = os.path.join(self.fresh("export"), "emoji.html")
        with open(path, "w", encoding="utf-8") as f:
            f.write(exported)
        b.open(pathlib.Path(path).as_uri(), ready="!!document.querySelector('.diffnote-comment__body')")
        self.assertTrue(b.js("[...document.querySelectorAll('.diffnote-comment__body')].some(e => e.textContent.includes('承認 👍 🎉'))"))

    def test_the_buttons_above_a_box_do_not_touch_it(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        b.click(f"{CUR} [data-diffnote-add=global]")
        gap = lambda box: b.js("(() => { const t = document.querySelector(%s); const bar = t.closest('form').querySelector('.diffnote-attach-bar').getBoundingClientRect(); return t.getBoundingClientRect().top - bar.bottom; })()" % json.dumps(box))
        composer = ".diffnote-compose textarea[placeholder^='コメントを書く']"
        self.assertTrue(b.wait_exists(composer))
        self.assertGreaterEqual(gap(composer), 3, "a new comment")
        self.assertGreaterEqual(gap(f"#{card} .diffnote-reply textarea"), 3, "a reply")

    def write_search(self, text):
        self.b.js("var t=document.querySelector('[data-diffnote-emoji-search]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(t,%s); t.dispatchEvent(new Event('input',{bubbles:true}))" % json.dumps(text))
        time.sleep(0.1)


class ReactionsToComments(ServedCase):
    def first(self, card):
        return f"#{card} [data-diffnote-comment]:not([data-diffnote-mine])"

    def add_through_the_table(self, comment, code):
        b = self.b
        b.click(f"{comment} [data-diffnote-emoji-button=react]")
        self.assertTrue(b.wait_exists("[data-diffnote-emoji-panel]"))
        b.click(f"[data-diffnote-emoji-panel] [data-diffnote-emoji='{code}']")

    def has_file(self):
        return "reactions.json" in harness.zip_names(self.review)

    def test_a_reaction_is_added_from_the_table_marked_as_mine_and_taken_back_by_pressing_it(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        comment = self.first(card)
        self.assertFalse(b.exists(f"{comment} [data-diffnote-reaction]"))
        self.add_through_the_table(comment, "tada")
        chip = f"{comment} [data-diffnote-reaction='🎉']"
        self.assertTrue(b.wait_exists(chip))
        self.assertEqual(b.text(chip), "🎉1")
        self.assertTrue(b.js(f"document.querySelector({json.dumps(chip)}).classList.contains('is-mine')"))
        self.assertIn("検証者", b.js(f"document.querySelector({json.dumps(chip)}).title"), "who")
        self.assertTrue(self.has_file(), "kept in the review")
        self.assertFalse(b.exists("[data-diffnote-emoji-panel]"), "the table is shut")
        # Pressed again: taken back, and nothing is kept.
        b.click(chip)
        self.assertTrue(b.wait(f"!document.querySelector({json.dumps(chip)})"))
        self.assertFalse(self.has_file())

    def test_the_same_reaction_of_another_name_counts_and_the_names_are_in_the_tip(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        comment = self.first(card)
        self.add_through_the_table(comment, "+1")
        chip = f"{comment} [data-diffnote-reaction='👍']"
        self.assertTrue(b.wait_exists(chip))
        # Signed in as another name (the user settings screen changes it).
        self.set_author_via_user_settings("別の人")
        self.addCleanup(lambda: os.path.exists(self.user_config_file()) and os.remove(self.user_config_file()))
        self.assertFalse(b.js(f"document.querySelector({json.dumps(chip)}).classList.contains('is-mine')"), "not this name's")
        b.click(chip)
        self.assertTrue(b.wait(f"document.querySelector({json.dumps(chip)}).textContent === '👍2'"))
        self.assertTrue(b.js(f"document.querySelector({json.dumps(chip)}).classList.contains('is-mine')"))
        tip = b.js(f"document.querySelector({json.dumps(chip)}).title")
        self.assertIn("検証者、別の人", tip)

    def test_a_reaction_goes_with_its_comment_when_that_is_deleted(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "消す返信")
        mine = f"#{card} [data-diffnote-mine]"
        self.add_through_the_table(mine, "heart")
        self.assertTrue(b.wait_exists(f"{mine} [data-diffnote-reaction='❤️']"))
        self.assertTrue(self.has_file())
        b.click(f"{mine} [data-diffnote-delete]")
        b.click(f"{mine} [data-diffnote-warn-ok]")
        self.assertTrue(b.wait("!document.body.textContent.includes('消す返信')"))
        self.assertFalse(self.has_file(), "the reaction went with it")

    def test_a_page_that_only_shows_the_review_shows_the_reactions_and_cannot_add(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.add_through_the_table(self.first(card), "rocket")
        self.assertTrue(b.wait_exists(f"{self.first(card)} [data-diffnote-reaction='🚀']"))
        exported = b.js("fetch('/export').then(r => r.text())")
        path = os.path.join(self.fresh("export"), "reactions.html")
        with open(path, "w", encoding="utf-8") as f:
            f.write(exported)
        b.open(pathlib.Path(path).as_uri(), ready="!!document.querySelector('[data-diffnote-reaction]')")
        self.assertEqual(b.js("document.querySelector('[data-diffnote-reaction]').textContent"), "🚀1")
        self.assertEqual(b.count("button[data-diffnote-reaction]"), 0, "only marks")
        self.assertFalse(b.exists("[data-diffnote-emoji-button]"), "nothing to add with")


class ThreadsOnFilesAndTheReview(ServedCase):
    def test_a_review_wide_thread_is_added_to_its_place(self):
        self.serve()
        b = self.b
        before = b.count(f"{CUR} [data-diffnote-global] .diffnote-thread")
        b.click(f"{CUR} [data-diffnote-add=global]")
        self.assertEqual(b.text(".diffnote-compose__where"), "レビュー全体へのコメント")
        self.write(".diffnote-compose textarea", "全体の方針について")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait(f"document.querySelectorAll('{CUR} [data-diffnote-global] .diffnote-thread').length === {before + 1} && !document.querySelector('.diffnote-compose-wrap')"))
        self.assertTrue(self.same_page())
        self.assertIn("全体の方針について", show(self.review))

    def test_a_table_and_struck_out_text_are_drawn_and_html_in_a_cell_is_text(self):
        self.serve()
        b = self.b
        b.click(f"{CUR} [data-diffnote-add=global]")
        self.write(".diffnote-compose textarea", "~~古い~~ 新しい\n\n|名前|値|\n|:-|-:|\n|x|1|\n|y|<b id=evil>|")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        body = f"{CUR} [data-diffnote-global] .diffnote-comment__body"
        self.assertTrue(b.wait(f"!!document.querySelector({json.dumps(body + ' table')})"))
        self.assertEqual(b.js(f"document.querySelector({json.dumps(body + ' del')}).textContent"), "古い")
        self.assertEqual(b.count(body + " table th"), 2)
        self.assertEqual(b.count(body + " table tr"), 3)
        self.assertEqual(b.js(f"document.querySelectorAll({json.dumps(body + ' table td')})[0].style.textAlign"), "left")
        self.assertEqual(b.js(f"document.querySelectorAll({json.dumps(body + ' table td')})[1].style.textAlign"), "right")
        self.assertFalse(b.exists("#evil"), "HTML in a cell is not an element")
        self.assertIn("<b id=evil>", b.js(f"document.querySelector({json.dumps(body + ' table')}).textContent"))

    def test_lines_of_a_file_that_is_new_can_be_commented_on(self):
        repo = os.path.join(self.root, "newfile")
        os.makedirs(repo)
        harness.git(repo, "init", "-q", "-b", "main")
        harness.write(repo, "old.txt", "old\n")
        harness.git(repo, "add", "-A")
        harness.git(repo, "commit", "-q", "-m", "c1")
        harness.git(repo, "tag", "c1")
        harness.write(repo, "new.txt", "".join(f"line {n}\n" for n in range(1, 8)))
        harness.git(repo, "add", "-A")
        harness.git(repo, "commit", "-q", "-m", "c2")
        harness.git(repo, "tag", "c2")
        master = os.path.join(self.root, "newfile.diffnote")
        assert harness.diffnote("edit", "-f", master, "--base", "c1", "c2", cwd=repo, comments=[("+line 1", "x")]).returncode == 0
        self.serve(master)
        b = self.b
        row = lambda n: f"{CUR} table[data-diffnote-file='new.txt'] tr[data-diffnote-new='{n}'] .diffnote-line__gutter-new"
        b.drag(row(4), row(7))
        self.assertTrue(b.wait_exists(".diffnote-compose"))
        self.write(".diffnote-compose textarea", "新しいファイルへ")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-compose-wrap')"))
        self.assertIn("new.txt:4-7", show(self.review))

    def test_a_file_thread_is_added_to_the_file_and_the_box_can_be_closed_with_escape(self):
        self.serve()
        b = self.b
        file = f"{CUR} section.diffnote-file[data-diffnote-file='calc.py']"
        b.click(f"{file} [data-diffnote-add=file]")
        self.assertEqual(b.text(".diffnote-compose__where"), "calc.py へのコメント")
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('.diffnote-compose-wrap')"))
        b.click(f"{file} [data-diffnote-add=file]")
        self.write(".diffnote-compose textarea", "ファイル全体について")
        b.js("document.querySelector('.diffnote-compose').requestSubmit()")
        cards = f"{file} > details > .diffnote-thread"
        self.assertTrue(b.wait(f"document.querySelectorAll({json.dumps(cards)}).length === 1"))
        self.assertEqual(b.text(cards + " .diffnote-thread__where"), "calc.py")
        self.assertTrue(any("ファイル全体: calc.py" in l for l in show(self.review).splitlines()))


class SideBySideLines(ServedCase):
    def serve_split(self):
        self.serve(self.login)
        self.b.js("localStorage.setItem('diffnote-layout','split')")
        self.b.reload()
        self.b.js("window.__marker='same-page'; window.__table=document.querySelector('.diffnote-diff')")
        self.assertTrue(self.b.exists(f"{CUR} {LOGIN}.diffnote-diff--split"))

    def gutter(self, side, n):
        return f"{CUR} {LOGIN} td.diffnote-line__gutter-{side}[data-diffnote-{side}='{n}']"

    def picked(self, side=None):
        cells = "td.is-picked"
        if side:
            cells += f".diffnote-cell--{'removed' if side == 'old' else 'added'}"
        return self.b.count(f"{CUR} {LOGIN} {cells}")

    def test_an_added_line_is_chosen_on_the_new_side_only(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("new", 10))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:10")
        self.assertEqual(self.picked(), 2, "its number and its text")
        self.assertEqual(b.count(f"{CUR} {LOGIN} .diffnote-cell--removed.is-picked"), 0)
        self.assertEqual(b.count(f"{CUR} {LOGIN} .diffnote-composer-row td[colspan='4']"), 1)
        self.write(".diffnote-composer-row textarea", "追加した行について")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(self.same_page())
        self.assertTrue(b.wait_exists(f"{CUR} {LOGIN} .diffnote-thread-row"))
        out = show(self.review)
        self.assertIn("追加した行について", out)
        line = [l for l in out.splitlines() if "login.ts:10" in l]
        self.assertTrue(line and "<-" not in line[0], out)

    def test_a_removed_line_is_chosen_on_the_old_side_only(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("old", 8))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:L8")
        self.assertEqual(self.picked("old"), 2)
        self.assertEqual(self.picked("new"), 0)
        self.write(".diffnote-composer-row textarea", "消した行について")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertTrue(any("login.ts:8" in l for l in show(self.review).splitlines()), show(self.review))

    def test_dragging_stays_on_the_side_it_started_on(self):
        self.serve_split()
        b = self.b
        b.drag(self.gutter("new", 9), self.gutter("new", 11))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:9-11")
        self.assertEqual(self.picked("new"), 6, "three lines, number and text")
        self.assertEqual(self.picked("old"), 0)
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked-first"), 2)
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked-last"), 2)
        b.escape()
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row') && !document.querySelector('.is-picked')"))

    def test_an_unchanged_line_is_on_both_sides_and_shift_click_extends_on_the_same_side(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("new", 2))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "src/auth/login.ts:2")
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked"), 2)
        b.click_at(self.gutter("new", 4), modifiers=8)
        self.assertTrue(b.wait("document.querySelector('.diffnote-compose__where').textContent==='src/auth/login.ts:2-4'"))
        self.write(".diffnote-composer-row textarea", "書きかけ")
        # Pressing the other side starts a new choice; the draft stays.
        b.click_at(self.gutter("old", 5))
        self.assertTrue(b.wait("document.querySelector('.diffnote-compose__where').textContent!=='src/auth/login.ts:2-4'"))
        self.assertEqual(b.count(f"{CUR} {LOGIN} td.is-picked"), 2, "one cell's number and text, on the old side")
        self.assertEqual(b.value(".diffnote-composer-row textarea"), "書きかけ")

    def test_cards_between_are_not_selected_along_with_one_side(self):
        self.serve_split()
        b = self.b
        b.js("document.querySelector(\"td.diffnote-line__gutter-new[data-diffnote-new='3']\").nextElementSibling.setAttribute('data-t','x')")
        b.drag("[data-t=x]", "[data-t=x]")
        # Everything in a card (its text, the reply box, the buttons) can't be selected.
        selectable = b.js("Array.from(document.querySelectorAll('.diffnote-diff--split .diffnote-thread-row, .diffnote-diff--split .diffnote-thread-row *')).filter(function(e){return getComputedStyle(e).userSelect!=='none'}).map(function(e){return e.tagName})")
        self.assertEqual(selectable, [])
        self.assertGreater(b.count(".diffnote-diff--split .diffnote-thread-row textarea"), 0)

    def test_changing_the_layout_lets_go_of_the_choice(self):
        self.serve_split()
        b = self.b
        b.click_at(self.gutter("new", 10))
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        b.click("[data-diffnote-layout=unified]")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row') && !document.querySelector('.is-picked, .diffnote-select')"))


class ExpandLeftOutLines(ServedCase):
    """The served page asks the server for the lines a diff leaves out."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.gaps, _ = make_gaps_review(cls.root)

    def test_the_lines_come_from_the_server_a_few_at_a_time_and_all_at_once(self):
        self.serve(self.gaps)
        b = self.b
        rows = lambda: b.js("Array.from(document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]')).map(function(r){return +r.getAttribute('data-diffnote-new')})")
        marker = lambda n: f"Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){{return r.textContent.includes('{n}')}})[0]"
        before = len(rows())
        b.js(marker(53) + ".querySelector('[data-diffnote-expand=top]').click()")
        self.assertTrue(b.wait(f"document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]').length === {before + 20}"))
        self.assertTrue(self.same_page())
        # The rest of that place at once.
        b.js(marker(33) + ".querySelector('[data-diffnote-expand=all]').click()")
        self.assertTrue(b.wait(f"document.querySelectorAll('.diffnote-diff tr[data-diffnote-new]').length === {before + 53}"))
        got = rows()
        self.assertEqual(got[got.index(23):got.index(77)], list(range(23, 77)))
        self.assertIn("row 50", b.text("table.diffnote-diff"))
        # Looking records nothing.
        self.assertEqual(entries(self.review), 3)

    def test_a_file_that_was_only_moved_is_one_place_the_server_fills_in(self):
        # Its diff is empty, so the whole file is the place to open, and the
        # served page has no lines of it until it asks for them.
        repo = os.path.join(self.fresh("moved"), "repo")
        os.makedirs(repo)
        harness.git(repo, "init", "-q", "-b", "main")
        harness.write(repo, "src/stays.py", "".join("row %d\n" % n for n in range(1, 31)))
        harness.write(repo, "touched.py", "x = 1\n")
        harness.git(repo, "add", "-A")
        harness.git(repo, "commit", "-q", "-m", "c1")
        harness.git(repo, "tag", "c1")
        os.makedirs(os.path.join(repo, "src", "deeper"))
        harness.git(repo, "mv", "src/stays.py", "src/deeper/stays.py")
        harness.write(repo, "touched.py", "x = 2\n")
        harness.git(repo, "add", "-A")
        harness.git(repo, "commit", "-q", "-m", "c2")
        harness.git(repo, "tag", "c2")
        review = os.path.join(self.fresh("moved-review"), "moved.diffnote")
        assert harness.diffnote("edit", "-f", review, "--base", "c1", "c2", cwd=repo,
                                comments=[("+x = 2", "ここだけ変更。")]).returncode == 0
        self.serve(review)
        b = self.b
        moved = f"{CUR} section.diffnote-file[data-diffnote-file='src/deeper/stays.py']"
        b.js("var d = document.querySelector(%s); d.open = true; d.dispatchEvent(new Event('toggle'))"
             % json.dumps(moved + " details"))
        self.assertTrue(b.wait_exists(moved + " [data-diffnote-expand='all']"))
        self.assertEqual(b.count(moved + " tr[data-diffnote-new]"), 0, "nothing of it is in the page yet")
        b.click(moved + " [data-diffnote-expand='all']")
        self.assertTrue(b.wait("document.querySelectorAll(%s).length === 30" % json.dumps(moved + " tr[data-diffnote-new]")))
        self.assertIn("row 30", b.text(moved + " table"))
        self.assertEqual(entries(self.review), 3, "looking records nothing")

    def test_a_comment_can_be_written_on_a_line_that_was_left_out_and_shown(self):
        self.serve(self.gaps)
        b = self.b
        b.js("Array.from(document.querySelectorAll('.diffnote-expand-row')).filter(function(r){return r.textContent.includes('53')})[0].querySelector('[data-diffnote-expand=all]').click()")
        self.assertTrue(b.wait("document.querySelectorAll('.diffnote-diff tr[data-diffnote-new=\"50\"]').length === 1"))
        b.click_at(f"{CUR} table[data-diffnote-file='long.txt'] tr[data-diffnote-new='50'] .diffnote-line__gutter-new")
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.assertEqual(b.text(".diffnote-compose__where"), "long.txt:50")
        self.write(".diffnote-composer-row textarea", "ここも気になります")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertIn("long.txt:50", show(self.review))
        # The thread brought its own context in; what was shown stays shown.
        self.assertTrue(b.wait_exists(f"{CUR} table[data-diffnote-file='long.txt'] .diffnote-thread-row"))
        self.assertTrue(b.exists("tr[data-diffnote-new='30']"))
        self.assertTrue(b.exists("tr[data-diffnote-new='70']"))


def git_short(repo, rev):
    return harness.git(repo, "rev-parse", "--short=7", rev)


class ServeAddsTheLatestDiff(ServedCase):
    """`init` on a commit, more commits, then `serve`: the changes since are there to review."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.repo = make_gaps_review(cls.root, name="since")[1]

    def start(self):
        self.review = os.path.join(self.fresh("review"), "since.diffnote")
        out = harness.diffnote("init", "-f", self.review, "c1", cwd=self.repo)
        assert out.returncode == 0, out.stdout + out.stderr
        self.server = Served(self.review, cwd=self.repo, author="検証者")
        self.addCleanup(self.server.stop)
        self.b = self.browser
        ready = "!!document.querySelector('.diffnote-file')"
        self.b.open(self.server.url, ready=ready)
        self.b.js("localStorage.setItem('diffnote-layout','unified')")
        self.b.reload(ready=ready)

    def test_the_commits_since_the_base_are_added_and_can_be_commented_on(self):
        self.start()
        b = self.b
        self.assertTrue(any("差分を記録しました" in l for l in self.server.said), self.server.said)
        self.assertEqual(entries(self.review), 3, "meta, the base, and the revision since")
        self.assertTrue(b.wait_exists("section.diffnote-file[data-diffnote-file='long.txt']"))
        b.js("document.querySelector('section.diffnote-file[data-diffnote-file=\"long.txt\"] details').open = true")
        b.js("document.querySelector('section.diffnote-file[data-diffnote-file=\"long.txt\"] details').dispatchEvent(new Event('toggle'))")
        self.assertTrue(b.wait_exists("tr[data-diffnote-new='20']"))
        b.click_at("tr[data-diffnote-new='20'] .diffnote-line__gutter-new")
        self.assertTrue(b.wait_exists(".diffnote-composer-row"))
        self.write(".diffnote-composer-row textarea", "ここを見てください")
        b.js("document.querySelector('.diffnote-composer-row .diffnote-compose').requestSubmit()")
        self.assertTrue(b.wait("!document.querySelector('.diffnote-composer-row')"))
        self.assertIn("long.txt:20", show(self.review))

    def test_with_no_commit_since_the_base_it_says_there_is_nothing_to_review_yet(self):
        review = os.path.join(self.fresh("review"), "none.diffnote")
        assert harness.diffnote("init", "-f", review, "HEAD", cwd=self.repo).returncode == 0
        server = Served(review, cwd=self.repo)
        self.addCleanup(server.stop)
        self.assertTrue(any("差分がまだありません" in n for n in server.notices), server.notices)
        self.assertEqual(entries(review), 2, "nothing was added")

    def open_page(self, server):
        self.server = server
        self.addCleanup(server.stop)
        self.b = self.browser
        ready = "!!document.querySelector('.diffnote-file')"
        self.b.open(server.url, ready=ready)

    def test_the_pull_button_takes_in_a_commit_made_after_the_server_started(self):
        repo = make_gaps_review(self.root, name="pulled")[1]
        review = os.path.join(self.fresh("review"), "pulled.diffnote")
        assert harness.diffnote("init", "-f", review, "c1", cwd=repo).returncode == 0
        self.open_page(Served(review, cwd=repo))
        b = self.b
        self.assertEqual(b.js("document.querySelectorAll('[data-diffnote-revision-link]').length"), 1, "one revision: its tab is shown")
        harness.write(repo, "long.txt", "".join(f"new {n}\n" for n in range(1, 101)))
        harness.git(repo, "commit", "-q", "-am", "c3")
        before = entries(review)
        self.assertFalse(b.exists("[data-diffnote-notice='pending']"), "nothing new yet")
        b.js("window.dispatchEvent(new Event('focus'))")
        self.assertTrue(b.wait_exists("[data-diffnote-notice='pending']"), "the new commit is told")
        self.assertEqual(entries(review), before, "telling doesn't take it in")
        # Pressing the notice opens the settings screen at 全般, where the
        # button (moved out of the topbar) now lives.
        b.click("[data-diffnote-notice='pending']")
        self.assertTrue(b.wait_exists("[data-diffnote-pull]"))
        self.assertEqual(self.settings_section(), "general")
        b.click("[data-diffnote-pull]")
        self.assertTrue(b.wait("document.querySelectorAll('[data-diffnote-revision-link]').length === 2"))
        self.assertIn("差分を記録しました", b.js("document.querySelector('[data-diffnote-pull-note]').textContent"))
        labels = b.js("[...document.querySelectorAll('[data-diffnote-revision-link]')].map(a => a.textContent).join('|')")
        self.assertRegex(labels, r"^#1 [0-9a-f]{7} \(\d+/\d\d \d\d:\d\d\)\|#2 [0-9a-f]{7} \(\d+/\d\d \d\d:\d\d\)$",
                         "the revisions are named by their commits, not the base")
        # (With a revision before it, the base is a menu whose first choice is the base;
        # this is part of the topbar, so it is visible with the settings screen open too.)
        self.assertTrue(b.js("document.querySelector('[data-diffnote-base]').textContent.startsWith('ベース: ')"))
        base = b.js("document.querySelector('[data-diffnote-base-select] option').textContent")
        self.assertEqual(base, git_short(repo, "c1"))
        self.assertGreater(entries(review), before)
        self.assertEqual(b.js("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink"), "1",
                         "what was taken in is shown")
        self.assertFalse(b.exists("[data-diffnote-notice='pending']"), "no longer new")
        b.click("[data-diffnote-pull]")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-pull-note]').textContent.includes('新しい変更はありません')"))
        self.assertEqual(b.js("document.querySelectorAll('[data-diffnote-revision-link]').length"), 2)

    def test_a_named_commit_has_nothing_to_pull(self):
        repo = make_gaps_review(self.root, name="named2")[1]
        review = os.path.join(self.fresh("review"), "named2.diffnote")
        assert harness.diffnote("init", "-f", review, "c1", cwd=repo).returncode == 0
        self.open_page(Served(review, cwd=repo, extra=["c2"]))
        harness.write(repo, "long.txt", "x\n")
        harness.git(repo, "commit", "-q", "-am", "c3")
        self.b.js("window.dispatchEvent(new Event('focus'))")
        time.sleep(0.5)
        self.assertFalse(self.b.exists("[data-diffnote-notice='pending']"))
        self.b.click("[data-diffnote-settings]")
        self.assertTrue(self.b.wait_exists("[data-diffnote-pull]"))
        self.b.click("[data-diffnote-pull]")
        self.assertTrue(self.b.wait("document.querySelector('[data-diffnote-pull-note]').textContent.includes('新しい変更はありません')"))

    def discard_all(self):
        b = self.b
        b.click("[data-diffnote-quit-more]")
        self.assertTrue(b.wait_exists("[data-diffnote-discard]"))
        b.click("[data-diffnote-discard]")
        self.assertTrue(b.wait_exists("[data-diffnote-discard-confirm]"))
        b.click("[data-diffnote-discard-confirm]")
        self.assertTrue(b.wait("!document.getElementById('app')"))

    def test_quitting_without_saving_also_takes_back_the_difference_serve_added(self):
        review = os.path.join(self.fresh("review"), "back.diffnote")
        assert harness.diffnote("init", "-f", review, "c1", cwd=self.repo).returncode == 0
        with open(review, "rb") as f:
            before = f.read()
        self.open_page(Served(review, cwd=self.repo))
        self.assertGreater(entries(review), 2, "the difference was added")
        self.discard_all()
        with open(review, "rb") as f:
            self.assertEqual(f.read(), before, "as if serve had not run")

    def test_quitting_without_saving_removes_a_bundle_that_serve_made(self):
        review = os.path.join(self.fresh("review"), "made.diffnote")
        self.open_page(Served(review, cwd=self.repo, extra=["--base", "c1", "c2"]))
        self.assertTrue(os.path.exists(review))
        self.discard_all()
        self.assertIn("削除しました", self.b.js("document.body.textContent"))
        self.assertFalse(os.path.exists(review))

    def test_a_base_and_a_target_make_the_bundle_if_there_is_none(self):
        review = os.path.join(self.fresh("review"), "named.diffnote")
        server = Served(review, cwd=self.repo, extra=["--base", "c1", "c2"])
        self.open_page(server)
        self.assertTrue(any("差分を記録しました" in l for l in server.said), server.said)
        self.assertTrue(os.path.exists(review))
        self.assertTrue(self.b.wait_exists("section.diffnote-file[data-diffnote-file='long.txt']"))
        self.assertGreaterEqual(entries(review), 2)

    def test_a_target_is_compared_with_the_base_of_the_bundle_and_not_added_twice(self):
        review = os.path.join(self.fresh("review"), "based.diffnote")
        assert harness.diffnote("init", "-f", review, "c1", cwd=self.repo).returncode == 0
        server = Served(review, cwd=self.repo, extra=["c2"])
        self.open_page(server)
        added = entries(review)
        self.assertEqual(added, 3, "meta, the base, and the target compared with it")
        server.stop()
        again = Served(review, cwd=self.repo, extra=["c2"])
        self.addCleanup(again.stop)
        self.assertFalse(any("差分を記録しました" in l for l in again.said), again.said)
        self.assertEqual(entries(review), added)

    def test_a_different_base_or_a_range_stops_serve_before_it_starts(self):
        review = os.path.join(self.fresh("review"), "bad.diffnote")
        assert harness.diffnote("init", "-f", review, "c1", cwd=self.repo).returncode == 0
        run = lambda *args: subprocess.run([harness.BIN, "serve", "-f", review, *args],
                                           cwd=self.repo, capture_output=True, text=True, encoding="utf-8", timeout=20)
        other = run("--base", "c2", "HEAD")
        self.assertNotEqual(other.returncode, 0)
        self.assertIn("ベース", other.stderr)
        ranged = run("c1..c2")
        self.assertNotEqual(ranged.returncode, 0)
        self.assertIn("範囲", ranged.stderr)
        self.assertEqual(entries(review), 2, "nothing was added")

    def test_a_directory_bundle_takes_the_directory_named_and_nothing_when_none_is(self):
        root = self.fresh("plain")
        with open(os.path.join(root, "a.txt"), "w") as f:
            f.write("one\ntwo\nthree\n")
        review = os.path.join(self.fresh("review"), "files.diffnote")
        assert harness.diffnote("init", "-f", review, cwd=root).returncode == 0
        with open(os.path.join(root, "a.txt"), "w") as f:
            f.write("one\nTWO\nthree\n")
        # No directory named: nothing is added (the directory is not known).
        quiet = Served(review, cwd=root)
        self.assertFalse(any("差分を記録しました" in l or "変更を記録しました" in l for l in quiet.said), quiet.said)
        quiet.stop()
        self.assertEqual(entries(review), 2)
        server = Served(review, cwd=root, extra=["."])
        self.open_page(server)
        self.assertTrue(any("ディレクトリの変更を記録しました" in l for l in server.said), server.said)
        self.assertEqual(entries(review), 3)
        self.assertTrue(self.b.wait_exists("section.diffnote-file[data-diffnote-file='a.txt']"))

    def test_two_directories_can_be_reviewed_in_one_step(self):
        old, new = self.fresh("old"), self.fresh("new")
        with open(os.path.join(old, "a.txt"), "w") as f:
            f.write("one\ntwo\nthree\n")
        with open(os.path.join(new, "a.txt"), "w") as f:
            f.write("one\nTWO\nthree\n")
        review = os.path.join(self.fresh("review"), "dirs.diffnote")
        server = Served(review, cwd=new, extra=["--files", "--base", old, "."])
        self.open_page(server)
        self.assertTrue(any("ディレクトリの変更を記録しました" in l for l in server.said), server.said)
        self.assertEqual(entries(review), 3, "meta, the base, and the directory compared with it")
        self.assertTrue(self.b.wait_exists("section.diffnote-file[data-diffnote-file='a.txt']"))
        # Nothing to compare: no bundle is left behind.
        same = self.fresh("same")
        with open(os.path.join(same, "a.txt"), "w") as f:
            f.write("one\ntwo\nthree\n")
        lone = os.path.join(self.fresh("review"), "none.diffnote")
        out = subprocess.run([harness.BIN, "serve", "-f", lone, "--files", "--base", old, "."],
                             cwd=same, capture_output=True, text=True, encoding="utf-8", timeout=20)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("差分がありません", out.stderr)
        self.assertFalse(os.path.exists(lone))

    def test_serving_again_with_nothing_new_adds_nothing(self):
        self.start()
        self.server.stop()
        before = entries(self.review)
        again = Served(self.review, cwd=self.repo, author="検証者")
        self.addCleanup(again.stop)
        self.assertFalse(any("差分を記録しました" in l for l in again.said), again.said)
        self.assertEqual(entries(self.review), before)


class Reopen(ServedCase):
    """`serve --reopen`: the last saved revision only, with no pull button and
    a later commit changing nothing it shows."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.repo = harness.make_gaps_review(cls.root, name="reopen-refuse")[1]

    def start_reopened(self, repo, review):
        self.repo = repo
        self.review = review
        self.server = Served(review, cwd=repo, extra=["--reopen"], author="検証者")
        self.addCleanup(self.server.stop)
        self.b = self.browser
        ready = "!!document.querySelector('.diffnote-file')"
        self.b.open(self.server.url, ready=ready)

    def test_nothing_is_added_at_start_and_no_pull_button_is_offered(self):
        repo = harness.make_gaps_review(self.root, name="reopen1")[1]
        review = os.path.join(self.fresh("review"), "reopen1.diffnote")
        assert harness.diffnote("init", "-f", review, "c1", cwd=repo).returncode == 0
        harness.set_user_author("reviewer")
        out = harness.diffnote("edit", "-f", review, "c2", cwd=repo, comments=[
            ("+TWENTY", "20 行目を変えました。"),
        ])
        assert out.returncode == 0, out.stdout + out.stderr
        before = entries(review)
        # A further commit, made after the review: --reopen never looks at it.
        harness.write(repo, "long.txt", "x\n")
        harness.git(repo, "commit", "-q", "-am", "c3")
        self.start_reopened(repo, review)
        b = self.b
        self.assertEqual(entries(review), before, "nothing recorded at startup")
        self.assertEqual(b.js("document.querySelectorAll('[data-diffnote-revision-link]').length"), 1)
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page]"))
        self.assertFalse(b.exists("[data-diffnote-pull]"), "no way to pull, even on 全般: reopen adds nothing, ever")
        b.click("[data-diffnote-settings-back]")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))
        b.js("window.dispatchEvent(new Event('focus'))")
        time.sleep(0.3)
        self.assertFalse(b.exists("[data-diffnote-notice='pending']"), "not even told about it")

    def test_a_reply_can_still_be_added_and_no_new_revision_appears(self):
        repo = harness.make_gaps_review(self.root, name="reopen2")[1]
        review = os.path.join(self.fresh("review"), "reopen2.diffnote")
        assert harness.diffnote("init", "-f", review, "c1", cwd=repo).returncode == 0
        harness.set_user_author("reviewer")
        out = harness.diffnote("edit", "-f", review, "c2", cwd=repo, comments=[
            ("+TWENTY", "20 行目を変えました。"),
        ])
        assert out.returncode == 0, out.stdout + out.stderr
        harness.write(repo, "long.txt", "x\n")
        harness.git(repo, "commit", "-q", "-am", "c3")
        self.start_reopened(repo, review)
        b = self.b
        b.js("localStorage.setItem('diffnote-layout','unified')")
        b.reload(ready="!!document.querySelector('.diffnote-file')")
        card = self.card("20 行目を変えました。")
        self.reply_to(card, "了解です")
        self.assertIn("了解です", show(review))
        self.assertEqual(b.js("document.querySelectorAll('[data-diffnote-revision-link]').length"), 1,
                         "the reply didn't add a revision")

    def test_reopen_with_a_comparison_target_is_refused_before_the_server_starts(self):
        review = os.path.join(self.fresh("review"), "refused.diffnote")
        out = subprocess.run([harness.BIN, "serve", "-f", review, "--reopen", "c2"],
                             cwd=self.repo, capture_output=True, text=True, encoding="utf-8", timeout=20)
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("--reopen", out.stderr)
        self.assertFalse(os.path.exists(review))


class BrowserHistory(ServedCase):
    """The browser's own back/forward buttons retrace revision switches,
    settings screens, and jumps (deep links).

    A short sleep sits before every `history.back()`/`forward()` and
    `b.escape()` here: the listener that hears them is attached by a
    `useEffect`, which runs a moment after the element it belongs to first
    appears (`wait_exists` only says the element is there). Firing the key
    or navigating sooner sometimes beats the listener to it."""

    SETTLE = 0.1

    def settle(self):
        time.sleep(self.SETTLE)

    def current_tab(self):
        return self.b.js("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink")

    def test_back_and_forward_retrace_revision_switches(self):
        self.serve()
        b = self.b
        first = self.current_tab()
        self.settle()
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === '0'"))
        self.settle()
        b.js("history.back()")
        self.assertTrue(b.wait(f"document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === {json.dumps(first)}"))
        self.settle()
        b.js("history.forward()")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === '0'"))

    def test_back_and_forward_retrace_opening_and_closing_the_settings_screens(self):
        self.serve()
        b = self.b
        self.assertFalse(b.exists("[data-diffnote-settings-page]"))
        self.settle()
        b.click("[data-diffnote-settings]")
        self.assertTrue(b.wait_exists("[data-diffnote-settings-page]"))
        self.assertEqual(self.settings_section(), "general")
        self.settle()
        b.click("[data-diffnote-user-settings]")
        self.assertTrue(b.wait("(function(){var p=document.querySelector('[data-diffnote-settings-page]'); return p && p.dataset.diffnoteSettingsSection === 'user'})()"))
        self.settle()
        b.js("history.back()")
        self.assertTrue(b.wait("(function(){var p=document.querySelector('[data-diffnote-settings-page]'); return p && p.dataset.diffnoteSettingsSection === 'general'})()"))
        self.settle()
        b.js("history.back()")
        self.assertTrue(b.wait("!document.querySelector('[data-diffnote-settings-page]')"))
        self.settle()
        b.js("history.forward()")
        self.assertTrue(b.wait("(function(){var p=document.querySelector('[data-diffnote-settings-page]'); return p && p.dataset.diffnoteSettingsSection === 'general'})()"))
        self.settle()
        b.js("history.forward()")
        self.assertTrue(b.wait("(function(){var p=document.querySelector('[data-diffnote-settings-page]'); return p && p.dataset.diffnoteSettingsSection === 'user'})()"))

    def test_back_and_forward_retrace_a_thread_jump_from_the_sidebar(self):
        self.serve()
        b = self.b
        original = self.current_tab()
        thread_id = b.js("document.querySelector('[data-diffnote-jump]').getAttribute('data-diffnote-jump')")
        hover = "document.querySelector(%s).classList.contains('diffnote-hover')" % json.dumps(f'[data-diffnote-thread-id="{thread_id}"]')
        self.settle()
        b.click(f"[data-diffnote-jump='{thread_id}']")
        self.assertTrue(b.wait(hover))
        # Elsewhere first, so back has something distinct to undo.
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === '0'"))
        self.settle()
        b.js("history.back()")
        self.assertTrue(b.wait(f"document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === {json.dumps(original)}"))
        self.assertTrue(b.wait(hover), "the jump (and its highlight) come back too")

    def test_back_and_forward_retrace_a_line_reference_clicked_in_a_comment(self):
        self.serve()
        b = self.b
        card = self.card("mul の型")
        self.reply_to(card, "calc.py:14 を見てください")
        self.settle()
        b.click("[data-diffnote-revision-link='0']")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === '0'"))
        b.click("[data-diffnote-revision-link='1']")
        self.assertTrue(b.wait_exists("[data-diffnote-lineref]"))
        b.click("[data-diffnote-lineref]")
        self.assertTrue(b.wait("!!document.querySelector('tr.diffnote-linked')"))
        # One step back: the jump itself is undone (the revision switch is a
        # step of its own, from before it).
        self.settle()
        b.js("history.back()")
        self.assertTrue(b.wait("!document.querySelector('tr.diffnote-linked')"))
        self.assertEqual(b.js("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink"), "1")
        self.settle()
        b.js("history.back()")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === '0'"))
        # Forward, twice: the switch, then the jump, both come back.
        self.settle()
        b.js("history.forward()")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === '1'"))
        self.settle()
        b.js("history.forward()")
        self.assertTrue(b.wait("!!document.querySelector('tr.diffnote-linked')"), "the jump is retraced too")

    def test_opening_a_url_with_a_hash_starts_there_directly(self):
        self.serve()
        b = self.b
        url = self.server.url.split('#')[0] + '#rev=1'
        b.open(url, ready="!!document.querySelector('.diffnote-file')")
        self.assertTrue(b.wait("document.querySelector('[data-diffnote-revision-link].is-current').dataset.diffnoteRevisionLink === '0'"))


if __name__ == "__main__":
    unittest.main()
