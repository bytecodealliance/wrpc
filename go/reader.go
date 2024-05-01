package wrpc

import (
	"context"
	"io"
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
		r.buffer = r.buffer[n:]
		return n, nil
	}
	select {
	case buf, ok := <-r.ch:
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
