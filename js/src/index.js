// wRPC for JavaScript: a transport-agnostic, component-native RPC
// implementation of the wRPC wire format and its mandatory framing.
//
// `types` (re-exported as `t`) builds the structural type tree; `value`
// encodes/decodes component-model values; `mux` implements the framing over a
// transport; and `session` invokes and serves functions with full
// `stream`/`future` support. `invoke`, `accept` and `serve` are the entry points.
// `frame` implements the pull-based framing over a byte stream pair, which
// `codec` transmits and receives values over.

export * as t from "./types.js";
export * as types from "./types.js";

export { Reader, Writer, AsyncReader, Chan, atomic } from "./io.js";
export { encodeValue, decodeValue, decodeSteps } from "./value.js";
export { DISCARDED, Mux, Outgoing, fromWebStreams } from "./mux.js";
export { PROTOCOL, FrameReader, FrameWriter, Framed, Listener } from "./frame.js";
export {
  UnreadError,
  decoder,
  encoder,
  errorDataAfterEnd,
  errorUnknownPath,
  receive,
  transmit,
} from "./codec.js";
export { MEDIA_TYPE, fromFetch, fromRequest } from "./http.js";
export { invoke, accept, serve } from "./session.js";
