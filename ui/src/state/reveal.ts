// Showing a line a diff leaves out before it is gone to (a search's hit):
// each file of the page says how it shows one of its own (`File.tsx`).
type Revealer = (line: number) => Promise<void>;

var revealers: Record<string, Revealer> = {};

function key(rev: number, path: string): string {
  return rev + '\0' + path;
}

/** What shows a line of `path` in revision `rev` (`null`: nothing any more). */
export function setRevealer(rev: number, path: string, revealer: Revealer | null): void {
  if (revealer) revealers[key(rev, path)] = revealer;
  else delete revealers[key(rev, path)];
}

/** Shows line `line` (new side) of `path`, if the diff leaves it out. */
export function reveal(rev: number, path: string, line: number): Promise<void> {
  var r = revealers[key(rev, path)];
  return r ? r(line) : Promise.resolve();
}
