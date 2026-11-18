package wrpc

import (
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
)

// frameVersion is the framing protocol version byte
const frameVersion = 0x01

const (
	maxInitialPathCapacity = 32
	defaultMaxFrameDepth   = 64
)

type Frame struct {
	Path []uint32
	Data []byte
}

type frameLimits struct {
	maxDepth uint32
}

// ReadFrameHeaderOption configures ReadFrameHeader
type ReadFrameHeaderOption func(*frameLimits)

// WithMaxFrameDepth sets the maximum path length of a frame read by ReadFrameHeader, which is 64 by default
func WithMaxFrameDepth(depth uint32) ReadFrameHeaderOption {
	return func(l *frameLimits) {
		l.maxDepth = depth
	}
}

// ReadFrameHeader reads the header of a frame from `r` and returns the frame path and the length
// of the frame data, which follows the header in `r`. It returns an unwrapped io.EOF if `r` ended
// before the frame
func ReadFrameHeader(r ByteReader, opts ...ReadFrameHeaderOption) ([]uint32, uint32, error) {
	l := frameLimits{
		maxDepth: defaultMaxFrameDepth,
	}
	for _, opt := range opts {
		opt(&l)
	}
	slog.Debug("reading path length")
	depth, err := ReadUint32(r)
	if err != nil {
		if err == io.EOF {
			return nil, 0, io.EOF
		}
		return nil, 0, fmt.Errorf("failed to read path length: %w", err)
	}
	if depth > l.maxDepth {
		return nil, 0, fmt.Errorf("path length of %d exceeds maximum of %d", depth, l.maxDepth)
	}
	path := make([]uint32, 0, min(depth, maxInitialPathCapacity))
	slog.Debug("reading path elements", "len", depth)
	for i := range depth {
		slog.Debug("reading path element", "index", i)
		v, err := ReadUint32(r)
		if err != nil {
			if err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return nil, 0, fmt.Errorf("failed to read path element %d: %w", i, err)
		}
		path = append(path, v)
	}
	slog.Debug("reading data length")
	n, err := ReadUint32(r)
	if err != nil {
		if err == io.EOF {
			err = io.ErrUnexpectedEOF
		}
		return nil, 0, fmt.Errorf("failed to read data length: %w", err)
	}
	return path, n, nil
}

func WriteFrame(v Frame, w ByteWriter) (int, error) {
	n := len(v.Path)
	if n > math.MaxUint32 {
		return 0, fmt.Errorf("path length of %d overflows a 32-bit integer", n)
	}
	slog.Debug("writing frame path length",
		"frame", v,
	)
	wn, err := WriteUint32(uint32(n), w)
	if err != nil {
		return wn, fmt.Errorf("failed to write path length of %d: %w", n, err)
	}
	slog.Debug("writing frame path elements",
		"frame", v,
	)
	for _, v := range v.Path {
		n, err = WriteUint32(v, w)
		if n > 0 {
			if math.MaxInt-n < wn {
				return math.MaxInt, errors.New("written byte count overflows int")
			}
			wn += n
		}
		if err != nil {
			return wn, fmt.Errorf("failed to write path contents: %w", err)
		}
	}
	slog.Debug("writing frame data",
		"frame", v,
	)
	n, err = WriteByteList(v.Data, w)
	if n > 0 {
		if math.MaxInt-n < wn {
			return math.MaxInt, errors.New("written byte count overflows int")
		}
		wn += n
	}
	if err != nil {
		return wn, fmt.Errorf("failed to write frame bytes: %w", err)
	}
	slog.Debug("written frame",
		"frame", v,
		"n", wn,
	)
	return wn, nil
}
