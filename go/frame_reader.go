package wrpc

import (
	"context"
	"encoding/binary"
	"errors"
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
	ch := make(chan []byte, 1)

	var nestMu sync.Mutex
	nest := make(map[string]chan []byte, len(paths))
	nest[""] = ch
	for _, path := range paths {
		p, err := subscribePath(path)
		if err != nil {
			slog.ErrorContext(ctx, "invalid subscription path", "err", err)
			continue
		}
		nest[p] = make(chan []byte)
	}
	var readers atomic.Int64
	readers.Add(1)

	go func() {
		defer func() {
			nestMu.Lock()
			for _, ch := range nest {
				close(ch)
			}
			nestMu.Unlock()

			if err := r.Close(); err != nil {
				slog.DebugContext(ctx, "failed to close frame stream reader", "err", err)
			}
		}()

	outer:
		for readers.Load() > 0 {
			slog.DebugContext(ctx, "reading frame")
			frame, err := ReadFrame(r)
			if err != nil {
				if err == io.EOF {
					break
				}
				slog.ErrorContext(ctx, "failed to read frame", "err", err)
				break
			}

			slog.DebugContext(ctx, "read frame",
				"frame", frame,
			)
			p := indexPath(frame.Path...)
			nestMu.Lock()
			ch, ok := nest[p]
			if !ok {
				slog.ErrorContext(ctx, "received a frame for unknown path", "path", frame.Path)
				nestMu.Unlock()
				break
			}
			if len(frame.Data) == 0 {
				slog.DebugContext(ctx, "received shutdown frame, closing channel", "path", frame.Path)
				delete(nest, p)
				close(ch)
			} else {
				slog.DebugContext(ctx, "received data frame, sending frame data",
					"frame", frame,
				)
				select {
				case <-ctx.Done():
					slog.DebugContext(ctx, "context done, stop reading frames",
						"err", ctx.Err(),
					)
					break outer
				case ch <- frame.Data:
					slog.DebugContext(ctx, "sent received frame data",
						"frame", frame,
					)
				}
			}
			nestMu.Unlock()
		}
	}()
	return newFrameStreamReader(&FrameStreamReader{
		ctx:     ctx,
		ch:      ch,
		readers: &readers,
		nestMu:  &nestMu,
		nest:    nest,
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
			return 0, io.ErrUnexpectedEOF
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
			return 0, io.ErrUnexpectedEOF
		}
		slog.DebugContext(r.ctx, "received data frame", "path", r.path)
		if len(buf) > 1 {
			r.buf = buf[1:]
		}
		return buf[0], nil
	}
}

func (r *FrameStreamReader) Index(path ...uint32) (IndexReadCloser, error) {
	p := indexPath(path...)
	readers := r.readers.Add(1)
	slog.DebugContext(r.ctx, "indexing reader",
		"readers", readers,
		"nested", r.nest,
		"path", p,
	)
	r.nestMu.Lock()
	defer r.nestMu.Unlock()
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

	delete(r.nest, r.path)
}

func (r *FrameStreamReader) Close() error {
	defer runtime.SetFinalizer(r, nil)
	r.drop()
	return nil
}
