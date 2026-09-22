// What the parts of the page are given without being handed it through every
// component between, and the shape of each.
//
// `null` on an exported page means "nothing can be changed": a component that
// is given one of these checks it before offering anything that would write.
import { createContext } from 'preact';
import type { At, LineRef } from '../lib.ts';
import type {
  Answer,
  AttachedData,
  FileData,
  Hunk,
  OpenedFile,
  Settings,
  Side,
  ViewModel,
} from '../model.ts';

/**
 * What the server answers a change with. Every one of them says whether it
 * worked; what else it carries depends on the change (the whole model, or only
 * what the page needs to put its own copy right).
 */
export type ChangeAnswer = Answer<{
  model?: ViewModel;
  stamp?: string;
  editable?: string[];
  changed?: string[];
  /** How long the log is now, and how much this change added to it. */
  events?: number;
  added?: number;
}>;

/** A new thread, as the page asks for it. */
export interface NewThread {
  kind: 'line' | 'file' | 'global';
  rev: number;
  path?: string;
  side?: Side;
  start?: number;
  end?: number;
  body?: string;
}

/** What the page can do to the review. */
export interface Actions {
  reply(id: string, text: string): Promise<ChangeAnswer>;
  create(request: NewThread): Promise<ChangeAnswer>;
  edit(id: string, text: string): Promise<ChangeAnswer>;
  remove(id: string): Promise<ChangeAnswer>;
  react(id: string, emoji: string): Promise<ChangeAnswer>;
  saveSettings(settings: Partial<Settings>): Promise<ChangeAnswer>;
  removeAttached(attached: AttachedData): Promise<ChangeAnswer>;
  saveUserSettings(author: string): Promise<ChangeAnswer>;
  refresh(): Promise<ChangeAnswer>;
  setResolved(id: string, resolved: boolean): Promise<ChangeAnswer>;
  /** The comments this page may rewrite or take out, and which it has. */
  editable: Set<string>;
  changed: Set<string>;
  /** The name comments are written under. */
  author?: string;
}

/** The lines being chosen, while they are being chosen. */
export interface Selection {
  rev: number;
  path: string;
  /** The side they are chosen on, in a side by side view; else none. */
  side?: Side | null;
  anchor: number;
  to: number;
}

/** A thread being written about a whole file, or the whole review. */
export interface Scope {
  kind: 'file' | 'global';
  rev: number;
  path?: string;
}

/** Choosing lines, and the box the new thread is written in. */
export interface Compose {
  sel: Selection | null;
  selecting: boolean;
  scope: Scope | null;
  draft: string;
  pending: boolean;
  error: string;
  setDraft(text: string): void;
  close(): void;
  begin(rev: number, path: string, idx: number, shift: boolean, side?: Side | null): void;
  /** A row's index, or what the row at that place on a side is. */
  extend(at: number | ((side: Side | null | undefined) => number | null)): void;
  openScope(kind: 'file' | 'global', rev: number, path?: string): void;
  send(request: NewThread): void;
}

/** The files opened to look at, which are in no revision of the review. */
export interface Opened {
  byRev: Record<number, OpenedFile[]>;
  open(rev: number, path: string): Promise<Answer<{ file?: OpenedFile }>>;
  close(rev: number, path: string): void;
  more(rev: number, path: string): Promise<Answer<{ hunk?: Hunk; next?: number | null }>>;
}

/** The files marked as looked at (one that has become another is not marked). */
export interface Viewed {
  is(file: FileData): boolean;
  toggle(file: FileData): void;
}

/** Where a jump can go, and what a comment's attachments are. */
export interface Links {
  /** Which revision is shown, and how many there are. */
  current: number;
  revisions: number;
  has(path: string): boolean;
  /** The most an attached file may weigh (the review's own rule). */
  limit: number;
  file(id: string, name: string): string;
  image(id: string): string;
  go(ref: LineRef | At): void;
  jump(rev: number, place: At): void;
}

/** How the diff is shown, and the ways to change it (the ⚙ menu). */
export interface View {
  wide: boolean;
  layout: 'unified' | 'split';
  resolved: number;
  interactive: boolean;
  hide: boolean;
  ignoreSpace: boolean;
  toggleSpace(on: boolean): void;
  setLayout(layout: 'unified' | 'split'): void;
  setHide(on: boolean): void;
}

export const ActionsContext = createContext<Actions | null>(null);
export const ComposeContext = createContext<Compose | null>(null);
export const OpenedContext = createContext<Opened | null>(null);
export const ViewedContext = createContext<Viewed | null>(null);
export const LinksContext = createContext<Links | null>(null);
export const ViewContext = createContext<View | null>(null);
