// How the page reaches the server, or `null` when there is none.
//
// Only the served page has one: `entry-serve.js` puts `api.js` here, and
// `entry-export.js` imports neither, so an exported page's bundle holds none of
// the code that would make a request (it is opened from a file). The app asks
// for `transport` and, where it may be `null`, treats that as "nothing to talk
// to" -- the same check it made of `Diffnote.api` before.
export let transport = null;

export function setTransport(api) {
  transport = api;
}
