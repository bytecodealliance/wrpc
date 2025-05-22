package wrpc

import (
	"errors"
	"fmt"
	"log/slog"
	"math"
)

func Slice[T any](v []T) *[]T {
	if v == nil {
		return nil
	}
	return &v
}

func WriteByteList(v []byte, w ByteWriter) (int, error) {
	n := len(v)
	if n > math.MaxUint32 {
		return 0, fmt.Errorf("byte list length of %d overflows a 32-bit integer", n)
	}
	slog.Debug("writing byte list length", "len", n)
	wn, err := WriteUint32(uint32(n), w)
	if err != nil {
		return wn, fmt.Errorf("failed to write list length of %d: %w", n, err)
	}
	slog.Debug("writing byte list contents")
	n, err = w.Write(v)
	if n > 0 {
		if math.MaxInt-n < wn {
			return math.MaxInt, errors.New("written byte count overflows int")
		}
		wn += n
	}
	if err != nil {
		return wn, fmt.Errorf("failed to write byte list contents: %w", err)
	}
	return wn, nil
}

func WriteList[T any](v []T, w ByteWriter, f func(T, ByteWriter) error) (int, error) {
	n := len(v)
	if n > math.MaxUint32 {
		return 0, fmt.Errorf("list length of %d overflows a 32-bit integer", n)
	}
	slog.Debug("writing list length", "len", n)
	wn, err := WriteUint32(uint32(n), w)
	if err != nil {
		return wn, fmt.Errorf("failed to write list length of %d: %w", n, err)
	}
	for i := range v {
		slog.Debug("writing list element", "index", i)
		if err := f(v[i], w); err != nil {
			return wn, fmt.Errorf("failed to write list element %d: %w", i, err)
		}
	}
	return wn, nil
}

// ReadByteList reads a []byte from `r` and returns it
func ReadByteList(r ByteReader) ([]byte, error) {
	slog.Debug("reading byte list length")
	n, err := ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read list length: %w", err)
	}

	b := make([]byte, n)
	slog.Debug("reading bytes", "len", n)
	rn, err := r.Read(b)
	if err != nil {
		return nil, fmt.Errorf("failed to read list bytes: %w", err)
	}
	if rn > int(n) {
		return nil, fmt.Errorf("invalid amount of list bytes read, expected %d, got %d", n, rn)
	}
	return b, nil
}

// ReadList reads a list from `r` and returns it
func ReadList[T any](r IndexReader, f func(IndexReader) (T, error)) ([]T, error) {
	slog.Debug("reading list length")
	n, err := ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read list length: %w", err)
	}
	vs := make([]T, n)
	slog.Debug("reading list elements", "len", n)
	for i := range vs {
		slog.Debug("reading list element", "index", i)
		v, err := f(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read list element %d: %w", i, err)
		}
		vs[i] = v
	}
	return vs, nil
}
