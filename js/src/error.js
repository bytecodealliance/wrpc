// The errors of reading headers and frames, receiving values, and invoking and
// serving functions, which callers match by class and `kind`. The error an
// error is caused by is its `cause`.

/** @param {unknown} cause */
const options = (cause) => (cause === undefined ? undefined : { cause });

/**
 * The error of decoding an invocation header.
 * @typedef {"unsupportedVersion" | "nameTooLong" | "invalidName" | "overflow" | "truncated" | "io"} HeaderDecodeErrorKind
 */
export class HeaderDecodeError extends Error {
  /**
   * @param {HeaderDecodeErrorKind} kind
   * @param {{ cause?: unknown, version?: number, len?: number, max?: number }} [details]
   */
  constructor(kind, { cause, ...details } = {}) {
    super(
      {
        unsupportedVersion: `unsupported version byte: ${details.version}`,
        nameTooLong: `name length of ${details.len} exceeds maximum of ${details.max}`,
        invalidName: "name is not valid UTF-8",
        overflow: "name length does not fit in a u32",
        truncated: "stream ended inside a header",
        io: "failed to read header",
      }[kind],
      options(cause),
    );
    this.name = "HeaderDecodeError";
    this.kind = kind;
    Object.assign(this, details);
  }
}

/**
 * The error of decoding a frame.
 * @typedef {"pathTooDeep" | "overflow" | "truncated" | "io"} FrameDecodeErrorKind
 */
export class FrameDecodeError extends Error {
  /**
   * @param {FrameDecodeErrorKind} kind
   * @param {{ cause?: unknown, depth?: number, max?: number }} [details]
   */
  constructor(kind, { cause, ...details } = {}) {
    super(
      {
        pathTooDeep: `path length of ${details.depth} exceeds maximum of ${details.max}`,
        overflow: "frame path or length does not fit in a u32",
        truncated: "stream ended inside a frame",
        io: "failed to read frame",
      }[kind],
      options(cause),
    );
    this.name = "FrameDecodeError";
    this.kind = kind;
    Object.assign(this, details);
  }
}

/**
 * The error of receiving a value.
 * @typedef {"unread" | "unknownPath" | "dataAfterEnd" | "truncated" | "decode" | "frame"} ReceiveErrorKind
 */
export class ReceiveError extends Error {
  /** @param {ReceiveErrorKind} kind @param {unknown} [cause] */
  constructor(kind, cause) {
    super(
      {
        unread: "received data for a cancelled consumer",
        unknownPath: "received frame on a path, which is not pending",
        dataAfterEnd: "received data after value end",
        truncated: "frames ended or a path was closed before its value was complete",
        decode: "failed to decode value",
        frame: "failed to read frames",
      }[kind],
      options(cause),
    );
    this.name = "ReceiveError";
    this.kind = kind;
  }
}

/** The error of serving an invocation, by the side at fault. */
export class ServeError extends Error {
  /** @param {"request" | "unread" | "handler" | "response"} kind @param {unknown} [cause] */
  constructor(kind, cause) {
    super(
      {
        request: "failed to receive request",
        unread: "handler dropped an unread parameter",
        handler: "handler failed",
        response: "failed to send response",
      }[kind],
      options(cause),
    );
    this.name = "ServeError";
    this.kind = kind;
  }
}

/** The error of invoking a function, by the side at fault. */
export class InvokeError extends Error {
  /** @param {"request" | "unread" | "response"} kind @param {unknown} [cause] */
  constructor(kind, cause) {
    super(
      {
        request: "failed to send request",
        unread: "client dropped an unread result",
        response: "failed to receive response",
      }[kind],
      options(cause),
    );
    this.name = "InvokeError";
    this.kind = kind;
  }
}
