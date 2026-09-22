// `htm` turns a template into elements; every component draws with it.
import { h } from 'preact';
import htm from 'htm';

export const html = htm.bind(h);
