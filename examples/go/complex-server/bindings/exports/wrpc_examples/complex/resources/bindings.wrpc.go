// Generated by `wit-bindgen-wrpc-go` 0.6.0. DO NOT EDIT!
package resources

import (
	bytes "bytes"
	context "context"
	binary "encoding/binary"
	errors "errors"
	fmt "fmt"
	wrpc "github.com/bytecodealliance/wrpc/go"
	io "io"
	slog "log/slog"
	math "math"
	utf8 "unicode/utf8"
)

type Foo interface{}
type Handler interface {
	Foo(ctx__ context.Context) (wrpc.Own[Foo], error)
	Foo_Foo(ctx__ context.Context, v wrpc.Own[Foo]) (string, error)
	Foo_Bar(ctx__ context.Context, self wrpc.Borrow[Foo]) (string, error)
	Bar(ctx__ context.Context, v wrpc.Borrow[Foo]) (string, error)
}

func ServeInterface(s wrpc.Server, h Handler) (stop func() error, err error) {
	stops := make([]func() error, 0, 4)
	stop = func() error {
		for _, stop := range stops {
			if err := stop(); err != nil {
				return err
			}
		}
		return nil
	}

	stop0, err := s.Serve("wrpc-examples:complex/resources", "foo", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer func() {
			if err := w.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close writer", "instance", "wrpc-examples:complex/resources", "name", "foo", "err", err)
			}
		}()
		slog.DebugContext(ctx, "calling `wrpc-examples:complex/resources.foo` handler")
		r0, err := h.Foo(ctx)
		if cErr := r.Close(); cErr != nil {
			slog.ErrorContext(ctx, "failed to close reader", "instance", "wrpc-examples:complex/resources", "name", "foo", "err", err)
		}
		if err != nil {
			slog.WarnContext(ctx, "failed to handle invocation", "instance", "wrpc-examples:complex/resources", "name", "foo", "err", err)
			return
		}

		var buf bytes.Buffer
		writes := make(map[uint32]func(wrpc.IndexWriter) error, 1)

		write0, err := (func(wrpc.IndexWriter) error)(nil), func(v string, w io.Writer) (err error) {
			n := len(v)
			if n > math.MaxUint32 {
				return fmt.Errorf("string byte length of %d overflows a 32-bit integer", n)
			}
			if err = func(v int, w io.Writer) error {
				b := make([]byte, binary.MaxVarintLen32)
				i := binary.PutUvarint(b, uint64(v))
				slog.Debug("writing string byte length", "len", n)
				_, err = w.Write(b[:i])
				return err
			}(n, w); err != nil {
				return fmt.Errorf("failed to write string byte length of %d: %w", n, err)
			}
			slog.Debug("writing string bytes")
			_, err = w.Write([]byte(v))
			if err != nil {
				return fmt.Errorf("failed to write string bytes: %w", err)
			}
			return nil
		}(string(r0), &buf)
		if err != nil {
			slog.WarnContext(ctx, "failed to write result value", "i", 0, "wrpc-examples:complex/resources", "name", "foo", "err", err)
			return
		}
		if write0 != nil {
			writes[0] = write0
		}
		slog.DebugContext(ctx, "transmitting `wrpc-examples:complex/resources.foo` result")
		_, err = w.Write(buf.Bytes())
		if err != nil {
			slog.WarnContext(ctx, "failed to write result", "wrpc-examples:complex/resources", "name", "foo", "err", err)
			return
		}
		if len(writes) > 0 {
			for index, write := range writes {
				w, err := w.Index(index)
				if err != nil {
					slog.ErrorContext(ctx, "failed to index writer", "index", index, "wrpc-examples:complex/resources", "name", "foo", "err", err)
					return
				}
				index := index
				write := write
				go func() {
					if err := write(w); err != nil {
						slog.WarnContext(ctx, "failed to write nested result value", "index", index, "wrpc-examples:complex/resources", "name", "foo", "err", err)
					}
				}()
			}
		}
	})
	if err != nil {
		return nil, fmt.Errorf("failed to serve `wrpc-examples:complex/resources.foo`: %w", err)
	}
	stops = append(stops, stop0)

	stop1, err := s.Serve("wrpc-examples:complex/resources", "foo.foo", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer func() {
			if err := w.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close writer", "instance", "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
			}
		}()
		slog.DebugContext(ctx, "reading parameter", "i", 0)
		p0, err := func(r interface {
			io.ByteReader
			io.Reader
		}) (wrpc.Own[Foo], error) {
			var x uint32
			var s uint
			for i := 0; i < 5; i++ {
				slog.Debug("reading owned resource ID length byte", "i", i)
				b, err := r.ReadByte()
				if err != nil {
					if i > 0 && err == io.EOF {
						err = io.ErrUnexpectedEOF
					}
					return "", fmt.Errorf("failed to read owned resource ID length byte: %w", err)
				}
				if b < 0x80 {
					if i == 4 && b > 1 {
						return "", errors.New("owned resource ID length overflows a 32-bit integer")
					}
					x = x | uint32(b)<<s
					buf := make([]byte, x)
					slog.Debug("reading owned resource ID bytes", "len", x)
					_, err = r.Read(buf)
					if err != nil {
						return "", fmt.Errorf("failed to read owned resource ID bytes: %w", err)
					}
					if !utf8.Valid(buf) {
						return "", errors.New("owned resource ID is not valid UTF-8")
					}
					return wrpc.Own[Foo](buf), nil
				}
				x |= uint32(b&0x7f) << s
				s += 7
			}
			return "", errors.New("owned resource ID length overflows a 32-bit integer")
		}(r)

		if err != nil {
			slog.WarnContext(ctx, "failed to read parameter", "i", 0, "instance", "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
			if err := r.Close(); err != nil {
				slog.ErrorContext(ctx, "failed to close reader", "instance", "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
			}
			return
		}
		slog.DebugContext(ctx, "calling `wrpc-examples:complex/resources.foo.foo` handler")
		r0, err := h.Foo_Foo(ctx, p0)
		if cErr := r.Close(); cErr != nil {
			slog.ErrorContext(ctx, "failed to close reader", "instance", "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
		}
		if err != nil {
			slog.WarnContext(ctx, "failed to handle invocation", "instance", "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
			return
		}

		var buf bytes.Buffer
		writes := make(map[uint32]func(wrpc.IndexWriter) error, 1)

		write0, err := (func(wrpc.IndexWriter) error)(nil), func(v string, w io.Writer) (err error) {
			n := len(v)
			if n > math.MaxUint32 {
				return fmt.Errorf("string byte length of %d overflows a 32-bit integer", n)
			}
			if err = func(v int, w io.Writer) error {
				b := make([]byte, binary.MaxVarintLen32)
				i := binary.PutUvarint(b, uint64(v))
				slog.Debug("writing string byte length", "len", n)
				_, err = w.Write(b[:i])
				return err
			}(n, w); err != nil {
				return fmt.Errorf("failed to write string byte length of %d: %w", n, err)
			}
			slog.Debug("writing string bytes")
			_, err = w.Write([]byte(v))
			if err != nil {
				return fmt.Errorf("failed to write string bytes: %w", err)
			}
			return nil
		}(r0, &buf)
		if err != nil {
			slog.WarnContext(ctx, "failed to write result value", "i", 0, "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
			return
		}
		if write0 != nil {
			writes[0] = write0
		}
		slog.DebugContext(ctx, "transmitting `wrpc-examples:complex/resources.foo.foo` result")
		_, err = w.Write(buf.Bytes())
		if err != nil {
			slog.WarnContext(ctx, "failed to write result", "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
			return
		}
		if len(writes) > 0 {
			for index, write := range writes {
				w, err := w.Index(index)
				if err != nil {
					slog.ErrorContext(ctx, "failed to index writer", "index", index, "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
					return
				}
				index := index
				write := write
				go func() {
					if err := write(w); err != nil {
						slog.WarnContext(ctx, "failed to write nested result value", "index", index, "wrpc-examples:complex/resources", "name", "foo.foo", "err", err)
					}
				}()
			}
		}
	})
	if err != nil {
		return nil, fmt.Errorf("failed to serve `wrpc-examples:complex/resources.foo.foo`: %w", err)
	}
	stops = append(stops, stop1)

	stop2, err := s.Serve("wrpc-examples:complex/resources", "foo.bar", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer func() {
			if err := w.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close writer", "instance", "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
			}
		}()
		slog.DebugContext(ctx, "reading parameter", "i", 0)
		p0, err := func(r interface {
			io.ByteReader
			io.Reader
		}) (wrpc.Borrow[Foo], error) {
			var x uint32
			var s uint
			for i := 0; i < 5; i++ {
				slog.Debug("reading borrowed resource ID length byte", "i", i)
				b, err := r.ReadByte()
				if err != nil {
					if i > 0 && err == io.EOF {
						err = io.ErrUnexpectedEOF
					}
					return "", fmt.Errorf("failed to read borrowed resource ID length byte: %w", err)
				}
				if b < 0x80 {
					if i == 4 && b > 1 {
						return "", errors.New("borrowed resource ID length overflows a 32-bit integer")
					}
					x = x | uint32(b)<<s
					buf := make([]byte, x)
					slog.Debug("reading borrowed resource ID bytes", "len", x)
					_, err = r.Read(buf)
					if err != nil {
						return "", fmt.Errorf("failed to read borrowed resource ID bytes: %w", err)
					}
					if !utf8.Valid(buf) {
						return "", errors.New("borrowed resource ID is not valid UTF-8")
					}
					return wrpc.Borrow[Foo](buf), nil
				}
				x |= uint32(b&0x7f) << s
				s += 7
			}
			return "", errors.New("borrowed resource ID length overflows a 32-bit integer")
		}(r)

		if err != nil {
			slog.WarnContext(ctx, "failed to read parameter", "i", 0, "instance", "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
			if err := r.Close(); err != nil {
				slog.ErrorContext(ctx, "failed to close reader", "instance", "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
			}
			return
		}
		slog.DebugContext(ctx, "calling `wrpc-examples:complex/resources.foo.bar` handler")
		r0, err := h.Foo_Bar(ctx, p0)
		if cErr := r.Close(); cErr != nil {
			slog.ErrorContext(ctx, "failed to close reader", "instance", "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
		}
		if err != nil {
			slog.WarnContext(ctx, "failed to handle invocation", "instance", "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
			return
		}

		var buf bytes.Buffer
		writes := make(map[uint32]func(wrpc.IndexWriter) error, 1)

		write0, err := (func(wrpc.IndexWriter) error)(nil), func(v string, w io.Writer) (err error) {
			n := len(v)
			if n > math.MaxUint32 {
				return fmt.Errorf("string byte length of %d overflows a 32-bit integer", n)
			}
			if err = func(v int, w io.Writer) error {
				b := make([]byte, binary.MaxVarintLen32)
				i := binary.PutUvarint(b, uint64(v))
				slog.Debug("writing string byte length", "len", n)
				_, err = w.Write(b[:i])
				return err
			}(n, w); err != nil {
				return fmt.Errorf("failed to write string byte length of %d: %w", n, err)
			}
			slog.Debug("writing string bytes")
			_, err = w.Write([]byte(v))
			if err != nil {
				return fmt.Errorf("failed to write string bytes: %w", err)
			}
			return nil
		}(r0, &buf)
		if err != nil {
			slog.WarnContext(ctx, "failed to write result value", "i", 0, "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
			return
		}
		if write0 != nil {
			writes[0] = write0
		}
		slog.DebugContext(ctx, "transmitting `wrpc-examples:complex/resources.foo.bar` result")
		_, err = w.Write(buf.Bytes())
		if err != nil {
			slog.WarnContext(ctx, "failed to write result", "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
			return
		}
		if len(writes) > 0 {
			for index, write := range writes {
				w, err := w.Index(index)
				if err != nil {
					slog.ErrorContext(ctx, "failed to index writer", "index", index, "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
					return
				}
				index := index
				write := write
				go func() {
					if err := write(w); err != nil {
						slog.WarnContext(ctx, "failed to write nested result value", "index", index, "wrpc-examples:complex/resources", "name", "foo.bar", "err", err)
					}
				}()
			}
		}
	})
	if err != nil {
		return nil, fmt.Errorf("failed to serve `wrpc-examples:complex/resources.foo.bar`: %w", err)
	}
	stops = append(stops, stop2)

	stop3, err := s.Serve("wrpc-examples:complex/resources", "bar", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
		defer func() {
			if err := w.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close writer", "instance", "wrpc-examples:complex/resources", "name", "bar", "err", err)
			}
		}()
		slog.DebugContext(ctx, "reading parameter", "i", 0)
		p0, err := func(r interface {
			io.ByteReader
			io.Reader
		}) (wrpc.Borrow[Foo], error) {
			var x uint32
			var s uint
			for i := 0; i < 5; i++ {
				slog.Debug("reading borrowed resource ID length byte", "i", i)
				b, err := r.ReadByte()
				if err != nil {
					if i > 0 && err == io.EOF {
						err = io.ErrUnexpectedEOF
					}
					return "", fmt.Errorf("failed to read borrowed resource ID length byte: %w", err)
				}
				if b < 0x80 {
					if i == 4 && b > 1 {
						return "", errors.New("borrowed resource ID length overflows a 32-bit integer")
					}
					x = x | uint32(b)<<s
					buf := make([]byte, x)
					slog.Debug("reading borrowed resource ID bytes", "len", x)
					_, err = r.Read(buf)
					if err != nil {
						return "", fmt.Errorf("failed to read borrowed resource ID bytes: %w", err)
					}
					if !utf8.Valid(buf) {
						return "", errors.New("borrowed resource ID is not valid UTF-8")
					}
					return wrpc.Borrow[Foo](buf), nil
				}
				x |= uint32(b&0x7f) << s
				s += 7
			}
			return "", errors.New("borrowed resource ID length overflows a 32-bit integer")
		}(r)

		if err != nil {
			slog.WarnContext(ctx, "failed to read parameter", "i", 0, "instance", "wrpc-examples:complex/resources", "name", "bar", "err", err)
			if err := r.Close(); err != nil {
				slog.ErrorContext(ctx, "failed to close reader", "instance", "wrpc-examples:complex/resources", "name", "bar", "err", err)
			}
			return
		}
		slog.DebugContext(ctx, "calling `wrpc-examples:complex/resources.bar` handler")
		r0, err := h.Bar(ctx, p0)
		if cErr := r.Close(); cErr != nil {
			slog.ErrorContext(ctx, "failed to close reader", "instance", "wrpc-examples:complex/resources", "name", "bar", "err", err)
		}
		if err != nil {
			slog.WarnContext(ctx, "failed to handle invocation", "instance", "wrpc-examples:complex/resources", "name", "bar", "err", err)
			return
		}

		var buf bytes.Buffer
		writes := make(map[uint32]func(wrpc.IndexWriter) error, 1)

		write0, err := (func(wrpc.IndexWriter) error)(nil), func(v string, w io.Writer) (err error) {
			n := len(v)
			if n > math.MaxUint32 {
				return fmt.Errorf("string byte length of %d overflows a 32-bit integer", n)
			}
			if err = func(v int, w io.Writer) error {
				b := make([]byte, binary.MaxVarintLen32)
				i := binary.PutUvarint(b, uint64(v))
				slog.Debug("writing string byte length", "len", n)
				_, err = w.Write(b[:i])
				return err
			}(n, w); err != nil {
				return fmt.Errorf("failed to write string byte length of %d: %w", n, err)
			}
			slog.Debug("writing string bytes")
			_, err = w.Write([]byte(v))
			if err != nil {
				return fmt.Errorf("failed to write string bytes: %w", err)
			}
			return nil
		}(r0, &buf)
		if err != nil {
			slog.WarnContext(ctx, "failed to write result value", "i", 0, "wrpc-examples:complex/resources", "name", "bar", "err", err)
			return
		}
		if write0 != nil {
			writes[0] = write0
		}
		slog.DebugContext(ctx, "transmitting `wrpc-examples:complex/resources.bar` result")
		_, err = w.Write(buf.Bytes())
		if err != nil {
			slog.WarnContext(ctx, "failed to write result", "wrpc-examples:complex/resources", "name", "bar", "err", err)
			return
		}
		if len(writes) > 0 {
			for index, write := range writes {
				w, err := w.Index(index)
				if err != nil {
					slog.ErrorContext(ctx, "failed to index writer", "index", index, "wrpc-examples:complex/resources", "name", "bar", "err", err)
					return
				}
				index := index
				write := write
				go func() {
					if err := write(w); err != nil {
						slog.WarnContext(ctx, "failed to write nested result value", "index", index, "wrpc-examples:complex/resources", "name", "bar", "err", err)
					}
				}()
			}
		}
	})
	if err != nil {
		return nil, fmt.Errorf("failed to serve `wrpc-examples:complex/resources.bar`: %w", err)
	}
	stops = append(stops, stop3)
	return stop, nil
}
