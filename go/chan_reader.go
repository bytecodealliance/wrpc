package wrpc

import (
	"context"
	"errors"
	"io"
	"log/slog"
	"math"
)

type ChanReader struct {
	ctx    context.Context
	ch     <-chan []byte
	buffer []byte
}

func NewChanReader(ctx context.Context, ch <-chan []byte, buffer []byte) *ChanReader {
	return &ChanReader{
		ctx, ch, buffer,
	}
}

func (r *ChanReader) Read(p []byte) (int, error) {
	if len(r.buffer) > 0 {
		n := copy(p, r.buffer)
		slog.Debug("copied bytes from buffer", "requested", len(p), "buffered", len(r.buffer), "copied", n)
		r.buffer = r.buffer[n:]
		for len(p) > n {
			slog.Debug("receiving next byte chunk", "blocking", false)
			select {
			case buf, ok := <-r.ch:
				slog.Debug("received byte chunk", "len", len(buf), "end", !ok)
				if !ok {
					return n, io.EOF
				}
				i := copy(p[n:], buf)
				r.buffer = buf[i:]
				if math.MaxInt-i > n {
					return math.MaxInt, errors.New("read byte count would overflow integer")
				}
				n += i
			default:
				return n, nil
			}
		}
		return n, nil
	}
	slog.Debug("receiving next byte chunk", "blocking", true)
	select {
	case buf, ok := <-r.ch:
		slog.Debug("received byte chunk", "len", len(buf), "end", !ok)
		if !ok {
			return 0, io.EOF
		}
		n := copy(p, buf)
		r.buffer = buf[n:]
		return n, nil
	case <-r.ctx.Done():
		return 0, r.ctx.Err()
	}
}

func (r *ChanReader) ReadByte() (byte, error) {
	if len(r.buffer) > 0 {
		b := r.buffer[0]
		slog.Debug("copied byte from buffer", "buffered", len(r.buffer))
		r.buffer = r.buffer[1:]
		return b, nil
	}
	for {
		select {
		case buf, ok := <-r.ch:
			if !ok {
				return 0, io.EOF
			}
			if len(buf) < 1 {
				continue
			}
			r.buffer = buf[1:]
			return buf[0], nil
		case <-r.ctx.Done():
			return 0, r.ctx.Err()
		}
	}
}
