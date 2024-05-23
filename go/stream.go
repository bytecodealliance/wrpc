package wrpc

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
)

type CompleteReader struct {
	io.Reader
}

func (*CompleteReader) IsComplete() bool {
	return true
}

func NewCompleteReader(r io.Reader) *CompleteReader {
	return &CompleteReader{r}
}

type CompleteByteReader struct {
	ByteReader
}

func (*CompleteByteReader) IsComplete() bool {
	return true
}

func NewCompleteByteReader(r ByteReader) *CompleteByteReader {
	return &CompleteByteReader{r}
}

type PendingReader struct {
	io.Reader
}

func (*PendingReader) IsComplete() bool {
	return false
}

func NewPendingReader(r io.Reader) *PendingReader {
	return &PendingReader{r}
}

type PendingByteReader struct {
	ByteReader
}

func (*PendingByteReader) IsComplete() bool {
	return false
}

func NewPendingByteReader(r ByteReader) *PendingByteReader {
	return &PendingByteReader{r}
}

type ByteStreamWriter struct {
	r     io.Reader
	chunk []byte
}

func (v *ByteStreamWriter) WriteTo(w ByteWriter) (err error) {
	if len(v.chunk) == 0 {
		v.chunk = make([]byte, 8096)
	}
	buf := bufio.NewWriter(w)
	defer func() {
		if fErr := buf.Flush(); fErr != nil {
			if err == nil {
				err = fmt.Errorf("failed to flush writer: %w", fErr)
			} else {
				slog.Warn("failed to flush writer", "err", fErr)
			}
		}
	}()
	for {
		var end bool
		slog.Debug("reading pending byte stream contents")
		n, err := v.r.Read(v.chunk)
		if err == io.EOF {
			end = true
			slog.Debug("pending byte stream reached EOF")
		} else if err != nil {
			return fmt.Errorf("failed to read pending byte stream chunk: %w", err)
		}
		if n > math.MaxUint32 {
			return fmt.Errorf("pending byte stream chunk length of %d overflows a 32-bit integer", n)
		}
		slog.Debug("writing pending byte stream chunk length", "len", n)
		if err := WriteUint32(uint32(n), buf); err != nil {
			return fmt.Errorf("failed to write pending byte stream chunk length of %d: %w", n, err)
		}
		_, err = buf.Write(v.chunk[:n])
		if err != nil {
			return fmt.Errorf("failed to write pending byte stream chunk contents: %w", err)
		}
		if end {
			if err := buf.WriteByte(0); err != nil {
				return fmt.Errorf("failed to write pending byte stream end byte: %w", err)
			}
			return nil
		}
	}
}

type ByteStreamReader struct {
	r   ByteReadCompleter
	buf uint32
}

func (r *ByteStreamReader) Read(p []byte) (int, error) {
	n := r.buf
	if n == 0 {
		slog.Debug("reading pending byte stream chunk length")
		var err error
		n, err = ReadUint32(r.r)
		if err != nil {
			return 0, fmt.Errorf("failed to read pending byte stream chunk length: %w", err)
		}
		if n == 0 {
			return 0, io.EOF
		}
	}
	if len(p) > int(n) {
		p = p[:n]
	}
	slog.Debug("reading pending byte stream chunk contents", "len", n)
	rn, err := r.r.Read(p)
	if err != nil {
		return rn, fmt.Errorf("failed to read pending stream chunk bytes: %w", err)
	}
	if rn > int(n) {
		return rn, errors.New("read more bytes than requested")
	}
	r.buf = n - uint32(rn)
	return rn, nil
}

func (r *ByteStreamReader) IsComplete() bool {
	return r.r.IsComplete()
}

func (r *ByteStreamReader) Close() error {
	c, ok := r.r.(io.Closer)
	if ok {
		return c.Close()
	}
	return nil
}

func NewByteStreamReader(r ByteReadCompleter) *ByteStreamReader {
	return &ByteStreamReader{
		r: r,
	}
}

// ReadStreamStatus reads a single byte from `r` and returns:
// - `true` if stream is "ready"
// - `false` if stream is "pending"
func ReadStreamStatus(r ByteReader) (bool, error) {
	status, err := r.ReadByte()
	if err != nil {
		return false, fmt.Errorf("failed to read `stream` status byte: %w", err)
	}
	switch status {
	case 0:
		return false, nil
	case 1:
		return true, nil
	default:
		return false, fmt.Errorf("invalid `stream` status byte %d", status)
	}
}

// ReadByteStream reads a stream of bytes from `r`
func ReadByteStream(r IndexReader, path ...uint32) (ReadCompleter, error) {
	slog.Debug("reading byte stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		r, err = r.Index(path...)
		if err != nil {
			return nil, fmt.Errorf("failed to index reader: %w", err)
		}
		return NewByteStreamReader(NewPendingByteReader(r)), nil
	}
	slog.Debug("reading ready byte stream")
	buf, err := ReadByteList(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read bytes: %w", err)
	}
	slog.Debug("read ready byte stream", "len", len(buf))
	return NewCompleteReader(bytes.NewReader(buf)), nil
}

// ReadStream reads a stream from `r`
func ReadStream[T any](r IndexReader, f func(IndexReader) (T, error), path ...uint32) (ReceiveCompleter[[]T], error) {
	slog.Debug("reading stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		r, err = r.Index(path...)
		if err != nil {
			return nil, fmt.Errorf("failed to index reader: %w", err)
		}
		return NewDecodeReceiver(r, func(r IndexReader) ([]T, error) {
			n, err := ReadUint32(r)
			if err != nil {
				return nil, fmt.Errorf("failed to read pending stream chunk length: %w", err)
			}
			if n == 0 {
				return nil, io.EOF
			}
			vs := make([]T, n)
			for i := range vs {
				v, err := f(r)
				if err != nil {
					return nil, fmt.Errorf("failed to read pending stream chunk element %d: %w", i, err)
				}
				vs[i] = v
			}
			return vs, nil
		}), nil
	}
	slog.Debug("reading ready stream")
	vs, err := ReadList(r, f)
	if err != nil {
		return nil, fmt.Errorf("failed to read ready stream: %w", err)
	}
	slog.Debug("read ready stream", "len", len(vs))
	return NewCompleteReceiver(vs), nil
}

func WriteByteStream(r ReadCompleter, w ByteWriter, chunk []byte, path ...uint32) (*ByteStreamWriter, error) {
	if r.IsComplete() {
		slog.Debug("writing byte stream `stream::ready` status byte")
		if err := w.WriteByte(1); err != nil {
			return nil, fmt.Errorf("failed to write `stream::ready` byte: %w", err)
		}
		var buf bytes.Buffer
		slog.Debug("reading ready byte stream contents")
		n, err := io.CopyBuffer(&buf, r, chunk)
		if err != nil {
			return nil, fmt.Errorf("failed to read ready byte stream contents: %w", err)
		}
		slog.Debug("writing ready byte stream contents", "len", n)
		return nil, WriteByteList(buf.Bytes(), w)
	}
	slog.Debug("writing byte stream `stream::pending` status byte")
	if err := w.WriteByte(0); err != nil {
		return nil, fmt.Errorf("failed to write `stream::pending` byte: %w", err)
	}
	return &ByteStreamWriter{r, chunk}, nil
}
