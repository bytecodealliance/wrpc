package wrpc

import (
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
	"slices"
	"unsafe"
)

const maxInitialCapacity = 1 << 20

// NewSlice returns an empty, non-nil slice with capacity for `n` elements,
// preallocating at most `maxInitialCapacity` bytes
func NewSlice[S ~[]E, E any](n uint32) S {
	var v E
	return make(S, 0, min(int(n), maxInitialCapacity/max(1, int(unsafe.Sizeof(v)))))
}

// ReadBytes reads exactly `n` bytes from `r` and returns them,
// preallocating at most `maxInitialCapacity` bytes up front
func ReadBytes(r io.Reader, n uint32) ([]byte, error) {
	b := make([]byte, 0, min(int(n), maxInitialCapacity))
	for len(b) < int(n) {
		k := min(int(n)-len(b), maxInitialCapacity)
		b = slices.Grow(b, k)
		if _, err := io.ReadFull(r, b[len(b):len(b)+k]); err != nil {
			if err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return nil, err
		}
		b = b[:len(b)+k]
	}
	return b, nil
}

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

	slog.Debug("reading bytes", "len", n)
	b, err := ReadBytes(r, n)
	if err != nil {
		return nil, fmt.Errorf("failed to read list bytes: %w", err)
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
	vs := NewSlice[[]T](n)
	slog.Debug("reading list elements", "len", n)
	for i := range n {
		slog.Debug("reading list element", "index", i)
		v, err := f(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read list element %d: %w", i, err)
		}
		vs = append(vs, v)
	}
	return vs, nil
}
