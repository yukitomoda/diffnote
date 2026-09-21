// Talking to the server that serves the page. Only the served page has this
// file: an exported page makes no requests (it is opened from a file).
//
// Every call gives back a promise of an object with `ok`; a failure to reach
// the server is `{ ok: false, error }` like the server's own refusals.
(function (D) {
  'use strict';

  var unreachable = { ok: false, error: 'サーバーに接続できませんでした' };
  var unreadable = { ok: false, error: '応答を読めませんでした' };

  function read(response) {
    return response.json().catch(function () {
      return unreadable;
    });
  }

  D.api = {
    // A change: JSON in, JSON out. The header is what makes the server
    // accept it (a page from another site can't send it).
    post: function (path, data) {
      return fetch(path, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Diffnote': '1' },
        credentials: 'same-origin',
        body: JSON.stringify(data || {}),
      }).then(read, function () {
        return unreachable;
      });
    },
    get: function (path) {
      return fetch(path, { credentials: 'same-origin' }).then(read, function () {
        return unreachable;
      });
    },
  };
})(window.Diffnote);
