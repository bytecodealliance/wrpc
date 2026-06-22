// The wRPC client for JavaScript: a transport-agnostic, component-native RPC
// implementation of the wRPC wire format and its mandatory framing.
//
// `types` (re-exported as `t`) builds the structural type tree; `value`
// encodes/decodes component-model values; `mux` implements the framing over a
// transport; and `session` invokes and serves functions with full
// `stream`/`future` support. `invoke` and `accept` are the entry points.

export * as t from "./types.js";
export * as types from "./types.js";

export { Reader, Writer, AsyncReader, Chan } from "./io.js";
export { encodeValue, decodeValue } from "./value.js";
export { Mux, Outgoing, PROTOCOL, fromWebStreams } from "./mux.js";
export { invoke, accept } from "./session.js";
