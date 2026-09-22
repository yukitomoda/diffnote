// Talking to the server that serves the page. Only the served page has this
// file: an exported page makes no requests (it is opened from a file).
//
// Every call gives back a promise of an object with `ok`; a failure to reach
// the server is `{ ok: false, error }` like the server's own refusals.
import { lib } from './lib.js';

// Functions, not constants: built lazily, since the message table (see
// lib.js) is only loaded once `start()` runs, after this file does.
var unreachable = function () {
  return { ok: false, error: lib.m('ui.api.unreachable') };
};
var unreadable = function () {
  return { ok: false, error: lib.m('ui.api.unreadable') };
};

function read(response) {
  return response.json().catch(function () {
    return unreadable();
  });
}

export const api = {
  // A change: JSON in, JSON out. The header is what makes the server
  // accept it (a page from another site can't send it).
  post: function (path, data) {
    return fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Diffnote': '1' },
      credentials: 'same-origin',
      body: JSON.stringify(data || {}),
    }).then(read, function () {
      return unreachable();
    });
  },
  // An image (a file or a pasted picture): its bytes, as they are.
  upload: function (blob) {
    return fetch('/api/images', {
      method: 'POST',
      headers: { 'Content-Type': blob.type || 'application/octet-stream', 'X-Diffnote': '1' },
      credentials: 'same-origin',
      body: blob,
    }).then(read, function () {
      return unreachable();
    });
  },
  // A file that is not a picture: its bytes, and what it is called.
  uploadFile: function (blob, name) {
    return fetch('/api/attachments?name=' + encodeURIComponent(name), {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream', 'X-Diffnote': '1' },
      credentials: 'same-origin',
      body: blob,
    }).then(read, function () {
      return unreachable();
    });
  },
  get: function (path) {
    return fetch(path, { credentials: 'same-origin' }).then(read, function () {
      return unreachable();
    });
  },
};
