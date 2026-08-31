// Hand-written mirror of crates/st-proto; task M4-05 replaces this with ts-rs
// generation, CI diff-checks it.
import { PROTO_VERSION, formatProtoVersion } from './control.js';

/** Wire form used in `hello` / `hello.ack` / `reject` (02 §2). */
export const PROTO_VERSION_STRING: string = formatProtoVersion(PROTO_VERSION);

/**
 * A peer is compatible when the majors are equal; the effective minor is the
 * minimum of the two (02 §2 rule 2 and 3).
 */
export function negotiateMinor(clientMinor: number, serverMinor: number): number {
  return Math.min(clientMinor, serverMinor);
}
