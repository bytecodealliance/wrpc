package wrpc

import (
	"bufio"
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
	nestMu  *sync.Mutex
	readers *atomic.Int64
	nest    map[string]chan []byte
	path    string
	buf     []byte
}

func newFrameStreamReader(r *FrameStreamReader) *FrameStreamReader {
	runtime.SetFinalizer(r, func(r *FrameStreamReader) {
		slog.DebugContext(r.ctx, "closing unused stream reader")
		if err := r.drop(); err != nil {
			slog.WarnContext(r.ctx, "failed to close stream reader", "err", err)
		}
	})
	return r
}

func NewFrameStreamReader(ctx context.Context, r io.ReadCloser, paths ...SubscribePath) *FrameStreamReader {
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

		r := bufio.NewReader(r)
		for readers.Load() > 0 {
			frame, err := ReadFrame(r)
			if err != nil {
				if err == io.EOF {
					break
				}
				slog.ErrorContext(ctx, "failed to read frame", "err", err)
				break
			}
			p := indexPath(frame.Path...)
			nestMu.Lock()
			ch, ok := nest[p]
			if !ok {
				slog.ErrorContext(ctx, "received a frame for unknown path", "path", frame.Path)
				nestMu.Unlock()
				break
			}
			if len(frame.Data) == 0 {
				close(ch)
			} else {
				ch <- frame.Data
			}
			nestMu.Unlock()
		}
	}()
	return newFrameStreamReader(&FrameStreamReader{
		ctx:     ctx,
		ch:      ch,
		nestMu:  &nestMu,
		readers: &readers,
		nest:    nest,
	})
}

func (r *FrameStreamReader) Read(p []byte) (int, error) {
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
	buf, ok := <-r.ch
	if !ok {
		return 0, io.ErrUnexpectedEOF
	}
	n := copy(p, buf)
	if n < len(buf) {
		r.buf = buf[n:]
	}
	return n, nil
}

func (r *FrameStreamReader) ReadByte() (byte, error) {
	if len(r.buf) > 0 {
		slog.DebugContext(r.ctx, "copied byte from buffer",
			"path", r.path,
			"buffered", len(r.buf),
		)
		b := r.buf[0]
		r.buf = r.buf[1:]
		return b, nil
	}
	buf, ok := <-r.ch
	if !ok {
		return 0, io.ErrUnexpectedEOF
	}
	if len(buf) > 1 {
		r.buf = buf[1:]
	}
	return buf[0], nil
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
		nestMu:  r.nestMu,
		readers: r.readers,
		nest:    r.nest,
		path:    p,
	}), nil
}

func (r *FrameStreamReader) drop() error {
	readers := r.readers.Add(-1)
	slog.DebugContext(r.ctx, "dropped stream reader",
		"readers", readers,
		"path", r.path,
	)
	r.nestMu.Lock()
	defer r.nestMu.Unlock()

	delete(r.nest, r.path)
	return nil
}

func (r *FrameStreamReader) Close() error {
	defer runtime.SetFinalizer(r, nil)
	return r.drop()
}
