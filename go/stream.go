package wrpc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
)

type ByteStreamWriter struct {
	r     io.Reader
	chunk []byte
}

func (v *ByteStreamWriter) WriteTo(w ByteWriter) error {
	if len(v.chunk) == 0 {
		v.chunk = make([]byte, 8096)
	}
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
		if err := WriteUint32(uint32(n), w); err != nil {
			return fmt.Errorf("failed to write pending byte stream chunk length of %d: %w", n, err)
		}
		_, err = w.Write(v.chunk[:n])
		if err != nil {
			return fmt.Errorf("failed to write pending byte stream chunk contents: %w", err)
		}
		if end {
			if err := w.WriteByte(0); err != nil {
				return fmt.Errorf("failed to write pending byte stream end byte: %w", err)
			}
			return nil
		}
	}
}

type byteStreamReceiver struct {
	ByteReader
	buffered uint32
}

func (r *byteStreamReceiver) Read(p []byte) (int, error) {
	n := r.buffered
	if n == 0 {
		slog.Debug("reading pending byte stream chunk length")
		var err error
		n, err = ReadUint32(r)
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
	rn, err := r.Read(p)
	if err != nil {
		return rn, fmt.Errorf("failed to read pending stream chunk bytes: %w", err)
	}
	if rn > int(n) {
		return rn, errors.New("read more bytes than requested")
	}
	r.buffered = n - uint32(rn)
	return rn, nil
}

func (*byteStreamReceiver) Ready() bool {
	return false
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

// ReadByteStream reads a stream of bytes from `r` and `ch`
func ReadByteStream(r IndexReader, path ...uint32) (ReadyReader, error) {
	slog.Debug("reading byte stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		r, err = r.Index(path...)
		if err != nil {
			return nil, fmt.Errorf("failed to get byte stream reader: %w", err)
		}
		return &byteStreamReceiver{r, 0}, nil
	}
	slog.Debug("reading ready byte stream")
	buf, err := ReadByteList(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read bytes: %w", err)
	}
	slog.Debug("read ready byte stream", "len", len(buf))
	return &byteReader{bytes.NewReader(buf)}, nil
}

// ReadStream reads a stream from `r` and `ch`
func ReadStream[T any](ctx context.Context, r ByteReader, ch <-chan []byte, f func(ByteReader) (T, error)) (ReadyReceiver[[]T], error) {
	slog.DebugContext(ctx, "reading stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, err
	}
	if !ok {
		return &decodeReceiver[[]T]{&ChanReader{ctx, ch, nil}, func(r ByteReader) ([]T, error) {
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
		}}, nil
	}
	slog.DebugContext(ctx, "reading ready stream")
	vs, err := ReadList(r, f)
	if err != nil {
		return nil, fmt.Errorf("failed to read ready stream: %w", err)
	}
	slog.DebugContext(ctx, "read ready stream", "len", len(vs))
	return &ready[[]T]{vs}, nil
}

func WriteByteStream(r ReadyReader, w ByteWriter, chunk []byte, path ...uint32) (*ByteStreamWriter, error) {
	if r.Ready() {
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
