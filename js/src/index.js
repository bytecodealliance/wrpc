// wRPC for JavaScript: a transport-agnostic, component-native RPC
// implementation of the wRPC wire format and its mandatory framing.
//
// `types` (re-exported as `t`) builds the structural type tree; `value`
// converts component-model values; `frame` implements the framing over a byte
// stream pair, which `codec` transmits and receives values over; `rpc`
// invokes and serves functions over it, as `http` does over HTTP; and `error`
// holds the errors of all of them.

export * as t from "./types.js";
export * as types from "./types.js";

export { Reader, Writer, atomic } from "./io.js";
export { PROTOCOL, FrameReader, FrameWriter, Framed, Listener } from "./frame.js";
export { decoder, encoder, pack, receive, transmit, unpack } from "./codec.js";
export {
  FrameDecodeError,
  HeaderDecodeError,
  InvokeError,
  ReceiveError,
  ServeError,
} from "./error.js";
export { invoke, serve } from "./rpc.js";
export { MEDIA_TYPE, IncomingRequest, accept, fromFetch } from "./http.js";
