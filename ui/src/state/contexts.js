// What the parts of the page are given without being handed it through every
// component between: `null` on an exported page means "nothing can be changed".
import { createContext } from 'preact';

// What the page can do to the review (`null` on an exported page): reply to
// a thread, resolve or reopen it. Set by the App, read by the cards.
export var ActionsContext = createContext(null);

// Choosing lines and writing a new thread (`null` on an exported page).
export var ComposeContext = createContext(null);

// Files opened to look at (`null` on an exported page).
export var OpenedContext = createContext(null);

// The files marked as looked at: `is(file)` and `toggle(file)` (a file that has
// become another one since is not marked any more).
export var ViewedContext = createContext(null);

// Places in a comment's text that name lines of a file: `has(path)` and
// `go(path, start, end)`.
export var LinksContext = createContext(null);

// How the diff is shown, and the ways to change it (the view menu).
export var ViewContext = createContext(null);
