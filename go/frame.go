package wrpc

import (
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
)

type Frame struct {
	Path []uint32
	Data []byte
}

// ReadFrame reads a single frame from `r` and returns it:
func ReadFrame(r ByteReader) (Frame, error) {
	slog.Debug("reading path length")
	n, err := ReadUint32(r)
	if err != nil {
		if err == io.EOF {
			return Frame{}, io.EOF
		}
		return Frame{}, fmt.Errorf("failed to read path length: %w", err)
	}
	path := make([]uint32, n)
	slog.Debug("reading path elements", "len", n)
	for i := range path {
		slog.Debug("reading path element", "index", i)
		v, err := ReadUint32(r)
		if err != nil {
			return Frame{}, fmt.Errorf("failed to read path element %d: %w", i, err)
		}
		path[i] = v
	}
	data, err := ReadByteList(r)
	if err != nil {
		return Frame{}, fmt.Errorf("failed to read data: %w", err)
	}
	return Frame{
		Path: path,
		Data: data,
	}, nil
}

func WriteFrame(v Frame, w ByteWriter) (int, error) {
	n := len(v.Path)
	if n > math.MaxUint32 {
		return 0, fmt.Errorf("path length of %d overflows a 32-bit integer", n)
	}
	slog.Debug("writing path length", "len", n)
	wn, err := WriteUint32(uint32(n), w)
	if err != nil {
		return wn, fmt.Errorf("failed to write path length of %d: %w", n, err)
	}
	slog.Debug("writing path elements")
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
	slog.Debug("writing frame data")
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
	return wn, nil
}
