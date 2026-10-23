package wrpc

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"runtime"
	"sync"
	"sync/atomic"
)

func indexPath(path ...uint32) string {
	p := make([]byte, len(path)*4)
	for i, v := range path {
		l := i * 4
		binary.LittleEndian.PutUint32(p[l:l+4], v)
	}
	return string(p)
}

func subscribePath(path SubscribePath) (string, error) {
	p := make([]byte, len(path)*4)
	for i, v := range path {
		if v == nil {
			return "", errors.New("wildcard subscriptions not supported yet")
		}
		l := i * 4
		binary.LittleEndian.PutUint32(p[l:l+4], *v)
	}
	return string(p), nil
}

type FrameStreamReader struct {
	ctx     context.Context
	ch      <-chan []byte
	readers *atomic.Int64
	nestMu  *sync.Mutex
	nest    map[string]chan []byte
	closed  map[string]struct{}
	dropped map[string]struct{}
	err     *atomic.Pointer[error]
	path    string
	buf     []byte
}

func newFrameStreamReader(r *FrameStreamReader) *FrameStreamReader {
	runtime.SetFinalizer(r, func(r *FrameStreamReader) {
		slog.DebugContext(r.ctx, "closing unused stream reader")
		r.drop()
	})
	return r
}

func NewFrameStreamReader(ctx context.Context, r ByteReadCloser, paths ...SubscribePath) *FrameStreamReader {
	ch := make(chan []byte, 64)

	var nestMu sync.Mutex
	nest := make(map[string]chan []byte, len(paths))
	nest[""] = ch
	for _, path := range paths {
		p, err := subscribePath(path)
		if err != nil {
			slog.ErrorContext(ctx, "invalid subscription path", "err", err)
			continue
		}
		nest[p] = make(chan []byte, 64)
	}
	var readers atomic.Int64
	readers.Add(1)

	closed := make(map[string]struct{})
	dropped := make(map[string]struct{})
	var ingressErr atomic.Pointer[error]
	go func() {
		defer func() {
			nestMu.Lock()
			for p, ch := range nest {
				if _, ok := closed[p]; !ok {
					close(ch)
				}
			}
			nestMu.Unlock()

			if err := r.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close frame stream reader", "err", err)
			}
		}()

		handle := func(frame Frame) error {
			p := indexPath(frame.Path...)
			nestMu.Lock()
			defer nestMu.Unlock()
			ch, ok := nest[p]
			if !ok {
				return fmt.Errorf("received a frame for unknown path `%v`", frame.Path)
			}
			if _, ok := closed[p]; ok {
				return fmt.Errorf("received a frame for closed path `%v`", frame.Path)
			}
			if _, ok := dropped[p]; ok && len(frame.Data) > 0 {
				return fmt.Errorf("received data for a closed reader on path `%v`", frame.Path)
			}
			if len(frame.Data) == 0 {
				slog.DebugContext(ctx, "received empty frame, closing path", "path", frame.Path)
				closed[p] = struct{}{}
				close(ch)
				return nil
			}
			slog.DebugContext(ctx, "received data frame, sending frame data",
				"frame", frame,
			)
			select {
			case <-ctx.Done():
				return ctx.Err()
			case ch <- frame.Data:
				slog.DebugContext(ctx, "sent received frame data",
					"frame", frame,
				)
				return nil
			default:
				return fmt.Errorf("reader too slow, dropped a frame for path `%v`", frame.Path)
			}
		}
		for readers.Load() > 0 {
			slog.DebugContext(ctx, "reading frame")
			frame, err := ReadFrame(r)
			if err == io.EOF {
				break
			}
			if err == nil {
				slog.DebugContext(ctx, "read frame",
					"frame", frame,
				)
				err = handle(frame)
			}
			if err != nil {
				slog.ErrorContext(ctx, "failed to process frame", "err", err)
				ingressErr.Store(&err)
				break
			}
		}
	}()
	return newFrameStreamReader(&FrameStreamReader{
		ctx:     ctx,
		ch:      ch,
		readers: &readers,
		nestMu:  &nestMu,
		nest:    nest,
		closed:  closed,
		dropped: dropped,
		err:     &ingressErr,
	})
}

func (r *FrameStreamReader) Read(p []byte) (int, error) {
	slog.DebugContext(r.ctx, "reading bytes",
		"path", r.path,
		"requested", len(p),
	)
	if len(r.buf) > 0 {
		n := copy(p, r.buf)
		slog.DebugContext(r.ctx, "copied bytes from buffer",
			"path", r.path,
			"requested", len(p),
			"buffered", len(r.buf),
			"copied", n,
		)
		r.buf = r.buf[n:]
		return n, nil
	}
	slog.DebugContext(r.ctx, "waiting for next frame",
		"path", r.path,
	)
	select {
	case <-r.ctx.Done():
		return 0, r.ctx.Err()
	case buf, ok := <-r.ch:
		if !ok {
			return 0, r.closedErr()
		}
		slog.DebugContext(r.ctx, "received data frame",
			"path", r.path,
			"buf", buf,
		)
		n := copy(p, buf)
		if n < len(buf) {
			r.buf = buf[n:]
		}
		return n, nil
	}
}

func (r *FrameStreamReader) ReadByte() (byte, error) {
	slog.DebugContext(r.ctx, "reading byte", "path", r.path)
	if len(r.buf) > 0 {
		b := r.buf[0]
		r.buf = r.buf[1:]
		slog.DebugContext(r.ctx, "copied byte from buffer",
			"path", r.path,
			"buffered", len(r.buf),
		)
		return b, nil
	}
	slog.DebugContext(r.ctx, "waiting for next frame", "path", r.path)
	select {
	case <-r.ctx.Done():
		return 0, r.ctx.Err()
	case buf, ok := <-r.ch:
		if !ok {
			return 0, r.closedErr()
		}
		slog.DebugContext(r.ctx, "received data frame", "path", r.path)
		if len(buf) > 1 {
			r.buf = buf[1:]
		}
		return buf[0], nil
	}
}

func (r *FrameStreamReader) Buffered() int {
	return len(r.buf)
}

func (r *FrameStreamReader) closedErr() error {
	r.nestMu.Lock()
	_, closed := r.closed[r.path]
	r.nestMu.Unlock()
	if err := r.err.Load(); err != nil && !closed {
		return *err
	}
	return io.EOF
}

func (r *FrameStreamReader) Index(path ...uint32) (IndexReadCloser, error) {
	p := r.path + indexPath(path...)
	readers := r.readers.Add(1)
	r.nestMu.Lock()
	defer r.nestMu.Unlock()
	slog.DebugContext(r.ctx, "indexing reader",
		"readers", readers,
		"nested", r.nest,
		"path", p,
	)
	ch, ok := r.nest[p]
	if !ok {
		return nil, errors.New("unknown subscription")
	}
	return newFrameStreamReader(&FrameStreamReader{
		ctx:     r.ctx,
		ch:      ch,
		readers: r.readers,
		nestMu:  r.nestMu,
		nest:    r.nest,
		closed:  r.closed,
		dropped: r.dropped,
		err:     r.err,
		path:    p,
	}), nil
}

func (r *FrameStreamReader) drop() {
	readers := r.readers.Add(-1)
	slog.DebugContext(r.ctx, "dropped frame stream reader",
		"readers", readers,
		"path", r.path,
	)

	r.nestMu.Lock()
	defer r.nestMu.Unlock()

	r.dropped[r.path] = struct{}{}
}

func (r *FrameStreamReader) Close() error {
	defer runtime.SetFinalizer(r, nil)
	r.drop()
	return nil
}
