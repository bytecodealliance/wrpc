package wrpc

import (
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
)

const (
	maxInitialPathCapacity = 32
	defaultMaxFrameDepth   = 64
	defaultMaxFrameSize    = 16 << 20
)

type Frame struct {
	Path []uint32
	Data []byte
}

type frameLimits struct {
	maxDepth uint32
	maxSize  uint32
}

// ReadFrameOption configures ReadFrame
type ReadFrameOption func(*frameLimits)

// WithMaxFrameDepth sets the maximum path length of a frame read by ReadFrame, which is 64 by default
func WithMaxFrameDepth(depth uint32) ReadFrameOption {
	return func(l *frameLimits) {
		l.maxDepth = depth
	}
}

// WithMaxFrameSize sets the maximum data length in bytes of a frame read by ReadFrame, which is 16 MiB by default
func WithMaxFrameSize(size uint32) ReadFrameOption {
	return func(l *frameLimits) {
		l.maxSize = size
	}
}

// ReadFrame reads a single frame from `r` and returns it
func ReadFrame(r ByteReader, opts ...ReadFrameOption) (Frame, error) {
	l := frameLimits{
		maxDepth: defaultMaxFrameDepth,
		maxSize:  defaultMaxFrameSize,
	}
	for _, opt := range opts {
		opt(&l)
	}
	depth, err := readFramePathLen(r)
	if err != nil {
		return Frame{}, err
	}
	if depth > l.maxDepth {
		return Frame{}, fmt.Errorf("path length of %d exceeds maximum of %d", depth, l.maxDepth)
	}
	path, n, err := readFramePath(r, depth, l.maxDepth)
	if err != nil {
		return Frame{}, err
	}
	if n > l.maxSize {
		return Frame{}, fmt.Errorf("data length of %d exceeds maximum of %d", n, l.maxSize)
	}
	data, err := ReadBytes(r, n)
	if err != nil {
		return Frame{}, fmt.Errorf("failed to read data: %w", err)
	}
	return Frame{
		Path: path,
		Data: data,
	}, nil
}

// readFramePathLen reads the path length of a frame from `r` and returns an unwrapped
// io.EOF if `r` ended before the frame
func readFramePathLen(r ByteReader) (uint32, error) {
	slog.Debug("reading path length")
	n, err := ReadUint32(r)
	if err != nil {
		if err == io.EOF {
			return 0, io.EOF
		}
		return 0, fmt.Errorf("failed to read path length: %w", err)
	}
	return n, nil
}

// readFramePath reads the `n` path elements of a frame, keeping at most `limit` of them,
// and the length of its data from `r`
func readFramePath(r ByteReader, n, limit uint32) ([]uint32, uint32, error) {
	path := make([]uint32, 0, min(n, limit, maxInitialPathCapacity))
	slog.Debug("reading path elements", "len", n)
	for i := range n {
		slog.Debug("reading path element", "index", i)
		v, err := ReadUint32(r)
		if err != nil {
			return nil, 0, fmt.Errorf("failed to read path element %d: %w", i, err)
		}
		if i < limit {
			path = append(path, v)
		}
	}
	slog.Debug("reading data length")
	k, err := ReadUint32(r)
	if err != nil {
		return nil, 0, fmt.Errorf("failed to read data length: %w", err)
	}
	return path, k, nil
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
