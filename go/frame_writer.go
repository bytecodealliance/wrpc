package wrpc

import (
	"bufio"
	"context"
	"errors"
	"io"
	"log/slog"
	"runtime"
	"sync/atomic"
)

var errWriterDone error = writerDoneError{}

type writerDoneError struct{}

func (writerDoneError) Error() string   { return "done writing frames" }
func (writerDoneError) Timeout() bool   { return false }
func (writerDoneError) Temporary() bool { return false }

type frameEgress struct {
	w      io.WriteCloser
	cancel context.CancelCauseFunc
	failed atomic.Bool
}

func (e *frameEgress) fail(err error) error {
	if e.failed.Swap(true) {
		return nil
	}
	e.cancel(err)
	return CloseWithError(e.w, err)
}

type FrameStreamWriter struct {
	ctx     context.Context
	egress  *frameEgress
	writers *atomic.Int64
	ch      chan<- Frame
	path    []uint32
	gone    atomic.Bool
	written bool
	closed  bool
}

func newFrameStreamWriter(w *FrameStreamWriter) *FrameStreamWriter {
	runtime.SetFinalizer(w, func(w *FrameStreamWriter) {
		slog.DebugContext(w.ctx, "closing unused stream writer")
		w.drop()
	})
	return w
}

func NewFrameStreamWriter(ctx context.Context, w io.WriteCloser) *FrameStreamWriter {
	ch := make(chan Frame, 64)
	var writers atomic.Int64
	writers.Add(1)

	ctx, cancel := context.WithCancelCause(ctx)
	egress := &frameEgress{w: w, cancel: cancel}
	go func() {
		fail := func(err error) {
			if err := egress.fail(err); err != nil {
				slog.DebugContext(ctx, "failed to close frame stream writer", "err", err)
			}
		}
		defer func() {
			if err := context.Cause(ctx); err != errWriterDone {
				fail(err)
				return
			}
			if err := w.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close frame stream writer", "err", err)
			}
		}()

		w := bufio.NewWriter(w)
	outer:
		for {
			select {
			case <-ctx.Done():
				return
			case frame, ok := <-ch:
				if !ok {
					break outer
				}
				_, err := WriteFrame(frame, w)
				if err != nil {
					slog.DebugContext(ctx, "failed to write frame", "err", err)
					fail(err)
					return
				}
			default:
				if err := w.Flush(); err != nil {
					slog.DebugContext(ctx, "failed to flush buffer", "err", err)
					fail(err)
					return
				}
				select {
				case <-ctx.Done():
					return
				case frame, ok := <-ch:
					if !ok {
						break outer
					}
					_, err := WriteFrame(frame, w)
					if err != nil {
						slog.DebugContext(ctx, "failed to write frame", "err", err)
						fail(err)
						return
					}
				}
			}
		}
		if err := w.Flush(); err != nil {
			slog.DebugContext(ctx, "failed to flush buffer", "err", err)
			fail(err)
			return
		}
		cancel(errWriterDone)
	}()
	return newFrameStreamWriter(&FrameStreamWriter{
		ctx:     ctx,
		egress:  egress,
		writers: &writers,
		ch:      ch,
		path:    nil,
	})
}

func sendFrame(ctx context.Context, ch chan<- Frame, frame Frame) error {
	select {
	case <-ctx.Done():
		return ctx.Err()
	case ch <- frame:
		slog.DebugContext(ctx, "sent transmit frame",
			"frame", frame,
		)
		return nil
	}
}

func (w *FrameStreamWriter) Write(p []byte) (int, error) {
	if len(p) == 0 {
		return 0, nil
	}
	data := make([]byte, len(p))
	copy(data, p)
	w.written = true
	if err := sendFrame(w.ctx, w.ch, Frame{
		Path: w.path,
		Data: data,
	}); err != nil {
		return 0, err
	}
	return len(p), nil
}

func (w *FrameStreamWriter) WriteBytes(p []byte) error {
	if len(p) == 0 {
		return nil
	}
	w.written = true
	return sendFrame(w.ctx, w.ch, Frame{
		Path: w.path,
		Data: p,
	})
}

func (w *FrameStreamWriter) WriteByte(c byte) error {
	w.written = true
	return sendFrame(w.ctx, w.ch, Frame{
		Path: w.path,
		Data: []byte{c},
	})
}

func (w *FrameStreamWriter) ClosePath() error {
	if w.closed {
		return nil
	}
	w.closed = true
	return sendFrame(w.ctx, w.ch, Frame{Path: w.path})
}

func (w *FrameStreamWriter) Index(path ...uint32) (IndexWriteCloser, error) {
	if len(path) == 0 {
		return nil, errors.New("path cannot be empty")
	}
	p := make([]uint32, 0, len(w.path)+len(path))
	p = append(p, w.path...)
	p = append(p, path...)
	writers := w.writers.Add(1)
	slog.DebugContext(w.ctx, "indexing writer",
		"writers", writers,
		"head", w.path,
		"tail", path,
		"path", p,
	)
	return newFrameStreamWriter(&FrameStreamWriter{
		ctx:     w.ctx,
		egress:  w.egress,
		writers: w.writers,
		ch:      w.ch,
		path:    p,
	}), nil
}

func (w *FrameStreamWriter) drop() bool {
	if w.gone.Swap(true) {
		return false
	}
	writers := w.writers.Add(-1)
	slog.DebugContext(w.ctx, "dropped frame stream writer",
		"writers", writers,
		"path", w.path,
	)
	if writers == 0 {
		if !w.egress.failed.Load() {
			// SAFETY: This is the last ref
			close(w.ch)
		}
		return true
	}
	return false
}

func (w *FrameStreamWriter) Close() error {
	defer runtime.SetFinalizer(w, nil)
	if (w.written || len(w.path) == 0) && !w.closed && !w.gone.Load() && !w.egress.failed.Load() {
		if err := w.ClosePath(); err != nil {
			slog.DebugContext(w.ctx, "failed to close path", "path", w.path, "err", err)
		}
	}
	if w.drop() {
		<-w.ctx.Done()
	}
	switch err := w.ctx.Err(); err {
	case nil:
		return nil
	case context.Canceled:
		err := context.Cause(w.ctx)
		if err == errWriterDone {
			return nil
		}
		return err
	default:
		return err
	}
}

// CloseWithError closes the writer like `Close` if `err` is nil. Otherwise, it fails all writers
// indexed from the same root and tears down the whole exchange (see `CloseWithError`).
func (w *FrameStreamWriter) CloseWithError(err error) error {
	if err == nil {
		return w.Close()
	}
	defer w.Close()
	return w.egress.fail(err)
}
