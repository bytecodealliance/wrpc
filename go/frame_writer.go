package wrpc

import (
	"bufio"
	"context"
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

type FrameStreamWriter struct {
	ctx     context.Context
	writers *atomic.Int64
	ch      chan<- Frame
	path    []uint32
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
	go func() {
		defer func() {
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
					cancel(err)
					return
				}
			default:
				if err := w.Flush(); err != nil {
					slog.DebugContext(ctx, "failed to flush buffer", "err", err)
					cancel(err)
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
						cancel(err)
						return
					}
				}
			}
		}
		if err := w.Flush(); err != nil {
			slog.DebugContext(ctx, "failed to flush buffer", "err", err)
			cancel(err)
			return
		}
		cancel(errWriterDone)
	}()
	return newFrameStreamWriter(&FrameStreamWriter{
		ctx:     ctx,
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
	if err := sendFrame(w.ctx, w.ch, Frame{
		Path: w.path,
		Data: p,
	}); err != nil {
		return 0, err
	}
	return len(p), nil
}

func (w *FrameStreamWriter) WriteByte(c byte) error {
	return sendFrame(w.ctx, w.ch, Frame{
		Path: w.path,
		Data: []byte{c},
	})
}

func (w *FrameStreamWriter) Index(path ...uint32) (IndexWriteCloser, error) {
	p := make([]uint32, 0, len(w.path))
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
		writers: w.writers,
		ch:      w.ch,
		path:    p,
	}), nil
}

func (w *FrameStreamWriter) drop() {
	writers := w.writers.Add(-1)
	slog.DebugContext(w.ctx, "dropped frame stream writer",
		"writers", writers,
		"path", w.path,
	)
	if writers == 0 {
		// SAFETY: This is the last ref
		close(w.ch)
		<-w.ctx.Done()
	}
}

func (w *FrameStreamWriter) Close() error {
	defer runtime.SetFinalizer(w, nil)
	w.drop()
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
