package wrpc

// HeaderDecodeErrorKind is the kind of a HeaderDecodeError
type HeaderDecodeErrorKind uint8

const (
	// HeaderUnsupportedVersion means the protocol version byte is not supported
	HeaderUnsupportedVersion HeaderDecodeErrorKind = iota
	// HeaderNameTooLong means the instance or function name is longer than the limit
	HeaderNameTooLong
	// HeaderInvalidName means the instance or function name is not valid UTF-8
	HeaderInvalidName
	// HeaderOverflow means a name length does not fit in a uint32
	HeaderOverflow
	// HeaderTruncated means the byte stream ended inside a header, Err is io.ErrUnexpectedEOF
	HeaderTruncated
	// HeaderIO means reading the byte stream failed, Err is the I/O error
	HeaderIO
)

// HeaderDecodeError is the error of decoding an invocation header
type HeaderDecodeError struct {
	Kind HeaderDecodeErrorKind
	Err  error
}

func (e *HeaderDecodeError) Error() string {
	var msg string
	switch e.Kind {
	case HeaderUnsupportedVersion:
		msg = "unsupported protocol version"
	case HeaderNameTooLong:
		msg = "header name too long"
	case HeaderInvalidName:
		msg = "header name is not valid UTF-8"
	case HeaderOverflow:
		msg = "header name length overflows uint32"
	case HeaderTruncated:
		msg = "byte stream ended inside a header"
	default:
		msg = "failed to read header"
	}
	return withCause(msg, e.Err)
}

func (e *HeaderDecodeError) Unwrap() error { return e.Err }

// FrameDecodeErrorKind is the kind of a FrameDecodeError
type FrameDecodeErrorKind uint8

const (
	// FramePathTooDeep means the path of a frame is longer than the limit
	FramePathTooDeep FrameDecodeErrorKind = iota
	// FrameOverflow means the path length, a path element or the data length does not fit in a uint32
	FrameOverflow
	// FrameTruncated means the byte stream ended inside a frame, Err is io.ErrUnexpectedEOF
	FrameTruncated
	// FrameIO means reading the byte stream failed, Err is the I/O error
	FrameIO
)

// FrameDecodeError is the error of decoding a frame
type FrameDecodeError struct {
	Kind FrameDecodeErrorKind
	Err  error
}

func (e *FrameDecodeError) Error() string {
	var msg string
	switch e.Kind {
	case FramePathTooDeep:
		msg = "frame path too deep"
	case FrameOverflow:
		msg = "frame integer overflows uint32"
	case FrameTruncated:
		msg = "byte stream ended inside a frame"
	default:
		msg = "failed to read frame"
	}
	return withCause(msg, e.Err)
}

func (e *FrameDecodeError) Unwrap() error { return e.Err }

// ReceiveErrorKind is the kind of a ReceiveError
type ReceiveErrorKind uint8

const (
	// ReceiveUnknownPath means a frame was received on a path, which is not pending
	ReceiveUnknownPath ReceiveErrorKind = iota
	// ReceiveDataAfterEnd means data was received after the end of a value or after its path was closed
	ReceiveDataAfterEnd
	// ReceiveTruncated means the frames ended before all paths were closed or a path was closed without a value,
	// Err wraps io.ErrUnexpectedEOF
	ReceiveTruncated
	// ReceiveDecode means a value decoder rejected the data, including a value cut short by its path closing,
	// Err is the decoder error
	ReceiveDecode
	// ReceiveFrame means reading frames failed, Err is a *FrameDecodeError
	ReceiveFrame
)

// ReceiveError is the error of receiving a value and the values pending in it
type ReceiveError struct {
	Kind ReceiveErrorKind
	Err  error
}

func (e *ReceiveError) Error() string {
	var msg string
	switch e.Kind {
	case ReceiveUnknownPath:
		msg = "received frame on a path that is not pending"
	case ReceiveDataAfterEnd:
		msg = "received data after the end of a value"
	case ReceiveTruncated:
		msg = "value is incomplete"
	case ReceiveDecode:
		msg = "failed to decode value"
	default:
		msg = "failed to receive frame"
	}
	return withCause(msg, e.Err)
}

func (e *ReceiveError) Unwrap() error { return e.Err }

// Fault is the side at fault for a failed invocation
type Fault uint8

const (
	// FaultRequest means transmitting or receiving the parameters failed
	FaultRequest Fault = iota
	// FaultHandler means the handler failed
	FaultHandler
	// FaultResponse means transmitting or receiving the results failed
	FaultResponse
)

// ServeError is the error of serving an invocation.
// Err is a *ReceiveError for FaultRequest, the handler error for FaultHandler
// and the error of encoding or transmitting the results for FaultResponse.
type ServeError struct {
	Fault Fault
	Err   error
}

func (e *ServeError) Error() string {
	switch e.Fault {
	case FaultRequest:
		return withCause("failed to receive parameters", e.Err)
	case FaultResponse:
		return withCause("failed to transmit results", e.Err)
	default:
		return withCause("handler failed", e.Err)
	}
}

func (e *ServeError) Unwrap() error { return e.Err }

// InvokeError is the error of an invocation.
// Err is the error of encoding or transmitting the header or the parameters for FaultRequest
// and a *ReceiveError for FaultResponse.
type InvokeError struct {
	Fault Fault
	Err   error
}

func (e *InvokeError) Error() string {
	if e.Fault == FaultRequest {
		return withCause("failed to transmit parameters", e.Err)
	}
	return withCause("failed to receive results", e.Err)
}

func (e *InvokeError) Unwrap() error { return e.Err }

func withCause(msg string, err error) string {
	if err == nil {
		return msg
	}
	return msg + ": " + err.Error()
}
