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

func writeFrames(ctx context.Context, w *bufio.Writer, frames ...Frame) error {
	for _, frame := range frames {
		_, err := WriteFrame(frame, w)
		if err != nil {
			slog.DebugContext(ctx, "failed to write frame", "err", err)
			return err
		}
		if err = w.Flush(); err != nil {
			slog.DebugContext(ctx, "failed to flush buffer", "err", err)
			return err
		}
	}
	return nil
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
		var frames []Frame
	outer:
		for {
			select {
			case <-ctx.Done():
				return
			case frame, ok := <-ch:
				if !ok {
					break outer
				}
				frames = append(frames, frame)
			default:
				if len(frames) == 0 {
					select {
					case <-ctx.Done():
						return
					case frame, ok := <-ch:
						if !ok {
							break outer
						}
						frames = append(frames, frame)
					}
				}
				if err := writeFrames(ctx, w, frames...); err != nil {
					cancel(err)
					return
				}
				frames = frames[:0]
			}
		}
		if err := writeFrames(ctx, w, frames...); err != nil {
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

func (w *FrameStreamWriter) Write(p []byte) (int, error) {
	select {
	case w.ch <- Frame{
		Path: w.path,
		Data: p,
	}:
	case <-w.ctx.Done():
		return 0, w.ctx.Err()
	}
	return len(p), nil
}

func (w *FrameStreamWriter) WriteByte(c byte) error {
	select {
	case w.ch <- Frame{
		Path: w.path,
		Data: []byte{c},
	}:
	case <-w.ctx.Done():
		return w.ctx.Err()
	}
	return nil
}

func (w *FrameStreamWriter) Index(path ...uint32) (IndexWriteCloser, error) {
	w.writers.Add(1)
	return newFrameStreamWriter(&FrameStreamWriter{
		ctx:     w.ctx,
		writers: w.writers,
		ch:      w.ch,
		path:    append(w.path, path...),
	}), nil
}

func (w *FrameStreamWriter) drop() {
	writers := w.writers.Add(-1)
	slog.DebugContext(w.ctx, "dropped stream writer",
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
