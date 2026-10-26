package wrpc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"sync"
)

const maxPendingStreamBatch = 1 << 20

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

// ReadByteStream reads a `stream<u8>` from `r`.
// The returned channel receives the chunks of the stream and is closed once the stream ends.
// If the stream is pending, the returned function reads it and must be called. If reading fails or `ctx`
// is done, the channel is never closed.
func ReadByteStream(r IndexReadCloser, path ...uint32) (<-chan []byte, func(context.Context) error, error) {
	slog.Debug("reading byte stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, nil, err
	}
	if ok {
		slog.Debug("reading ready byte stream")
		buf, err := ReadByteList(r)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to read ready byte stream: %w", err)
		}
		ch := make(chan []byte, 1)
		if len(buf) > 0 {
			ch <- buf
		}
		close(ch)
		return ch, nil, nil
	}
	if len(path) > 0 {
		slog.Debug("indexing pending byte stream reader")
		r, err = r.Index(path...)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to index nested byte stream reader: %w", err)
		}
	}
	ch := make(chan []byte)
	return ch, func(ctx context.Context) error {
		if len(path) > 0 {
			defer closeNested(r)
		}
		for {
			slog.Debug("reading pending byte stream chunk")
			buf, err := ReadByteList(r)
			if err != nil {
				return fmt.Errorf("failed to read pending byte stream chunk: %w", err)
			}
			if len(buf) == 0 {
				slog.Debug("pending byte stream ended")
				if r.Buffered() > 0 {
					return errors.New("received data after pending byte stream end")
				}
				close(ch)
				return nil
			}
			select {
			case ch <- buf:
			case <-ctx.Done():
				return fmt.Errorf("failed to send pending byte stream chunk: %w", context.Cause(ctx))
			}
		}
	}, nil
}

// ReadStream reads a `stream` from `r` using `f` to read its elements.
// The returned channel receives the chunks of the stream and is closed once the stream ends.
// If the stream is pending, the returned function reads it and must be called. If reading fails or `ctx`
// is done, the channel is never closed.
func ReadStream[T any](r IndexReadCloser, f func(IndexReadCloser, ...uint32) (T, func(context.Context) error, error), path ...uint32) (<-chan []T, func(context.Context) error, error) {
	slog.Debug("reading stream status byte")
	ok, err := ReadStreamStatus(r)
	if err != nil {
		return nil, nil, err
	}
	if ok {
		slog.Debug("reading ready stream length")
		n, err := ReadUint32(r)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to read ready stream length: %w", err)
		}
		vs := NewSlice[[]T](n)
		var ds []func(context.Context) error
		for i := range n {
			v, deferred, err := f(r, append(path, i)...)
			if err != nil {
				return nil, nil, fmt.Errorf("failed to read ready stream element %d: %w", i, err)
			}
			vs = append(vs, v)
			if deferred != nil {
				ds = append(ds, deferred)
			}
		}
		ch := make(chan []T, 1)
		if len(vs) > 0 {
			ch <- vs
		}
		close(ch)
		return ch, JoinDeferred(ds...), nil
	}
	if len(path) > 0 {
		slog.Debug("indexing pending stream reader")
		r, err = r.Index(path...)
		if err != nil {
			return nil, nil, fmt.Errorf("failed to index nested stream reader: %w", err)
		}
	}
	ch := make(chan []T)
	return ch, func(ctx context.Context) error {
		if len(path) > 0 {
			defer closeNested(r)
		}
		var g errGroup
		err := func() error {
			var total uint32
			for {
				slog.Debug("reading pending stream chunk length")
				n, err := ReadUint32(r)
				if err != nil {
					return fmt.Errorf("failed to read pending stream chunk length: %w", err)
				}
				if n == 0 {
					slog.Debug("pending stream ended")
					if r.Buffered() > 0 {
						return errors.New("received data after pending stream end")
					}
					close(ch)
					return nil
				}
				if math.MaxUint32-n < total {
					return errors.New("total pending stream element count would overflow a 32-bit unsigned integer")
				}
				vs := NewSlice[[]T](n)
				for range n {
					v, deferred, err := f(r, total)
					if err != nil {
						return fmt.Errorf("failed to read pending stream element %d: %w", total, err)
					}
					if deferred != nil {
						g.Go(func() error { return deferred(ctx) })
					}
					vs = append(vs, v)
					total++
				}
				select {
				case ch <- vs:
				case <-ctx.Done():
					return fmt.Errorf("failed to send pending stream chunk: %w", context.Cause(ctx))
				}
			}
		}()
		return g.Wait(err)
	}, nil
}

// WriteByteStream writes a `stream<u8>` with chunks received from `v` to `w`
func WriteByteStream(v <-chan []byte, w ByteWriter) (func(IndexWriter) error, error) {
	if v == nil {
		return nil, errors.New("outgoing byte stream channel is nil")
	}
	slog.Debug("writing `stream::pending` status byte")
	if err := w.WriteByte(0); err != nil {
		return nil, fmt.Errorf("failed to write `stream::pending` byte: %w", err)
	}
	return func(w IndexWriter) error {
		return writeStreamChunks(v, w, func(chunk []byte, buf *bytes.Buffer) error {
			slog.Debug("encoding pending byte stream chunk", "len", len(chunk))
			if _, err := WriteByteList(chunk, buf); err != nil {
				return fmt.Errorf("failed to encode pending byte stream chunk: %w", err)
			}
			return nil
		})
	}, nil
}

// WriteStream writes a `stream` with chunks received from `v` to `w` using `f` to write its elements
func WriteStream[T any](v <-chan []T, w ByteWriter, f func(T, ByteWriter) (func(IndexWriter) error, error)) (func(IndexWriter) error, error) {
	if v == nil {
		return nil, errors.New("outgoing stream channel is nil")
	}
	slog.Debug("writing `stream::pending` status byte")
	if err := w.WriteByte(0); err != nil {
		return nil, fmt.Errorf("failed to write `stream::pending` byte: %w", err)
	}
	return func(w IndexWriter) error {
		var g errGroup
		var total uint32
		err := writeStreamChunks(v, w, func(chunk []T, buf *bytes.Buffer) error {
			n := len(chunk)
			if n > math.MaxUint32 || math.MaxUint32-uint32(n) < total {
				return errors.New("total pending stream element count would overflow a 32-bit unsigned integer")
			}
			slog.Debug("encoding pending stream chunk", "len", n)
			if _, err := WriteUint32(uint32(n), buf); err != nil {
				return fmt.Errorf("failed to encode pending stream chunk length of %d: %w", n, err)
			}
			for _, v := range chunk {
				write, err := f(v, buf)
				if err != nil {
					return fmt.Errorf("failed to encode pending stream element %d: %w", total, err)
				}
				if write != nil {
					w, err := w.Index(total)
					if err != nil {
						return fmt.Errorf("failed to index nested stream writer: %w", err)
					}
					g.Go(func() error { return write(w) })
				}
				total++
			}
			return nil
		})
		return g.Wait(err)
	}, nil
}

func writeStreamChunks[T any](v <-chan []T, w IndexWriter, f func([]T, *bytes.Buffer) error) error {
	bufs := make(chan []byte)
	writeErr := make(chan error, 1)
	go func() {
		defer close(writeErr)
		for buf := range bufs {
			if err := w.WriteBytes(buf); err != nil {
				writeErr <- fmt.Errorf("failed to write pending stream chunks: %w", err)
				return
			}
		}
	}()
	var buf bytes.Buffer
	err := func() error {
		for {
			recv := v
			var send chan<- []byte
			if buf.Len() > 0 {
				send = bufs
				if buf.Len() >= maxPendingStreamBatch {
					recv = nil
				}
			}
			select {
			case chunk, ok := <-recv:
				if !ok {
					slog.Debug("writing pending stream end")
					buf.WriteByte(0)
					select {
					case bufs <- buf.Bytes():
						return nil
					case err := <-writeErr:
						return err
					}
				}
				if len(chunk) == 0 {
					continue
				}
				if err := f(chunk, &buf); err != nil {
					return err
				}
			case send <- buf.Bytes():
				buf = bytes.Buffer{}
			case err := <-writeErr:
				return err
			}
		}
	}()
	close(bufs)
	if err != nil {
		go drain(v)
		return err
	}
	return <-writeErr
}

func closeNested(r IndexReadCloser) {
	if err := r.Close(); err != nil {
		slog.Debug("failed to close nested reader", "err", err)
	}
}

func drain[T any](v <-chan T) {
	for range v {
	}
}

type errGroup struct {
	wg   sync.WaitGroup
	mu   sync.Mutex
	errs []error
}

func (g *errGroup) Go(f func() error) {
	g.wg.Go(func() {
		if err := f(); err != nil {
			g.mu.Lock()
			g.errs = append(g.errs, err)
			g.mu.Unlock()
		}
	})
}

func (g *errGroup) Wait(err error) error {
	g.wg.Wait()
	return errors.Join(append(g.errs, err)...)
}
