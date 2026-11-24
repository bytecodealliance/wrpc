package wrpc

import (
	"context"
	"io"
	"slices"
)

// Receiver receives the asynchronous values pending in a decoded value
type Receiver interface {
	// Receive takes `data` of a frame on `path`, relative to the value, an empty `data` closes `path`.
	// It returns once all of `data` was taken, which may wait for the consumer of a pending value.
	Receive(ctx context.Context, path []uint32, data []byte) error
	// Complete returns whether the paths of all values pending in the value are closed
	Complete() bool
}

// Transmitter transmits the asynchronous values pending in an encoded value
type Transmitter interface {
	// Transmit writes the frames of the values pending in the value on `path` to `w`.
	// It returns once all of their paths are closed.
	Transmit(ctx context.Context, w *FrameWriter, path []uint32) error
}

// Receivers routes the frames on path `[i, ..]` to the Receiver at index `i`
type Receivers map[uint32]Receiver

func (rs Receivers) Receive(ctx context.Context, path []uint32, data []byte) error {
	if len(path) == 0 {
		return errUnknownPath(path)
	}
	r, ok := rs[path[0]]
	if !ok {
		return errUnknownPath(path)
	}
	if err := r.Receive(ctx, path[1:], data); err != nil {
		return err
	}
	if r.Complete() {
		delete(rs, path[0])
	}
	return nil
}

func (rs Receivers) Complete() bool {
	return len(rs) == 0
}

// Transmitters transmits the Transmitter at index `i` on path `[.., i]`, all of them concurrently
type Transmitters map[uint32]Transmitter

func (ts Transmitters) Transmit(ctx context.Context, w *FrameWriter, path []uint32) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	errs := make(chan error, len(ts))
	for i, t := range ts {
		path := append(slices.Clip(path), i)
		go func() { errs <- t.Transmit(ctx, w, path) }()
	}
	for range ts {
		if err := <-errs; err != nil {
			return err
		}
	}
	return nil
}

func receivers(rxs ...Receiver) Receiver {
	rs := Receivers{}
	for i, rx := range rxs {
		if rx != nil {
			rs[uint32(i)] = rx
		}
	}
	if len(rs) == 0 {
		return nil
	}
	return rs
}

func transmitters(txs ...Transmitter) Transmitter {
	ts := Transmitters{}
	for i, tx := range txs {
		if tx != nil {
			ts[uint32(i)] = tx
		}
	}
	if len(ts) == 0 {
		return nil
	}
	return ts
}

// pending is the Receiver of a pending value, whose data `run` decodes in a goroutine started by the first frame.
// `run` reads until io.EOF, i.e. until the path is closed, and registers the values nested in the value in `nested`.
type pending struct {
	run    func(ctx context.Context, r ByteReader, nested Receivers) error
	nested Receivers
	r      *pathReader
	closed bool
}

func newPending(run func(context.Context, ByteReader, Receivers) error) *pending {
	return &pending{run: run, nested: Receivers{}}
}

func (p *pending) Receive(ctx context.Context, path []uint32, data []byte) error {
	if len(path) > 0 {
		return p.nested.Receive(ctx, path, data)
	}
	if p.closed {
		return errDataAfterEnd
	}
	if p.r == nil {
		p.r = &pathReader{ctx: ctx, data: make(chan []byte), done: make(chan error)}
		go func() {
			err := p.run(ctx, p.r, p.nested)
			select {
			case p.r.done <- err:
			case <-ctx.Done():
			}
		}()
	}
	if len(data) == 0 {
		p.closed = true
		close(p.r.data)
	} else {
		select {
		case p.r.data <- data:
		case <-ctx.Done():
			return context.Cause(ctx)
		}
	}
	select {
	case err := <-p.r.done:
		return err
	case <-ctx.Done():
		return context.Cause(ctx)
	}
}

func (p *pending) Complete() bool {
	return p.closed && p.nested.Complete()
}

// pathReader reads the data handed over by pending, which it reports as taken once it needs more
type pathReader struct {
	ctx    context.Context
	data   chan []byte
	done   chan error
	buffer []byte
	read   bool
	eof    bool
}

func (r *pathReader) fill() error {
	for len(r.buffer) == 0 {
		if r.eof {
			return io.EOF
		}
		if r.read {
			select {
			case r.done <- nil:
			case <-r.ctx.Done():
				return context.Cause(r.ctx)
			}
		}
		select {
		case b, ok := <-r.data:
			if !ok {
				r.eof = true
				return io.EOF
			}
			r.buffer = b
			r.read = true
		case <-r.ctx.Done():
			return context.Cause(r.ctx)
		}
	}
	return nil
}

func (r *pathReader) ReadByte() (byte, error) {
	if err := r.fill(); err != nil {
		return 0, err
	}
	b := r.buffer[0]
	r.buffer = r.buffer[1:]
	return b, nil
}

func (r *pathReader) Read(p []byte) (int, error) {
	if err := r.fill(); err != nil {
		return 0, err
	}
	n := copy(p, r.buffer)
	r.buffer = r.buffer[n:]
	return n, nil
}

// rootReader reads the data of the root path, failing on frames on other paths
type rootReader struct {
	r      *FrameReader
	closed bool
}

func (r *rootReader) fill() error {
	for r.r.n == 0 {
		if r.closed {
			return io.EOF
		}
		path, n, err := r.r.Next()
		if err != nil {
			return unexpectedEOF(err)
		}
		if len(path) > 0 {
			return errUnknownPath(path)
		}
		if n == 0 {
			r.closed = true
		}
	}
	return nil
}

func (r *rootReader) ReadByte() (byte, error) {
	if err := r.fill(); err != nil {
		return 0, err
	}
	return r.r.ReadByte()
}

func (r *rootReader) Read(p []byte) (int, error) {
	if err := r.fill(); err != nil {
		return 0, err
	}
	return r.r.Read(p)
}

// Receive receives a value from the frames read by `r`, decoding it using `c`.
// It returns the value and the Receipt receiving the asynchronous values pending in it.
func Receive[T any](r *FrameReader, c Codec[T]) (T, *Receipt, error) {
	var zero T
	rr := &rootReader{r: r}
	v, rx, err := c.Decode(rr)
	if err != nil {
		return zero, nil, unexpectedEOF(err)
	}
	if r.n > 0 {
		return zero, nil, errDataAfterEnd
	}
	return v, &Receipt{r: r, rx: rx, closed: rr.closed}, nil
}

// Receipt receives the asynchronous values pending in a received value
type Receipt struct {
	r      *FrameReader
	rx     Receiver
	closed bool
}

// Wait receives frames and hands their data to the pending values until all paths are closed.
// It stalls while the consumer of a pending value does not take its data.
func (rc *Receipt) Wait(ctx context.Context) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	for !rc.closed || rc.rx != nil && !rc.rx.Complete() {
		path, n, err := rc.r.Next()
		if err == io.EOF {
			return errIngressEnded
		}
		if err != nil {
			return err
		}
		if len(path) == 0 {
			if rc.closed || n > 0 {
				return errDataAfterEnd
			}
			rc.closed = true
			continue
		}
		if rc.rx == nil {
			return errUnknownPath(path)
		}
		if n == 0 {
			if err := rc.rx.Receive(ctx, path, nil); err != nil {
				return err
			}
			continue
		}
		for rc.r.n > 0 {
			b, err := rc.r.peek()
			if err != nil {
				return err
			}
			if err := rc.rx.Receive(ctx, path, b); err != nil {
				return err
			}
			rc.r.discard(len(b))
		}
	}
	return nil
}

// Transmit encodes `v` using `c` and writes it to `w`.
// It returns the Transmission of the asynchronous values pending in `v`.
func Transmit[T any](w *FrameWriter, c Codec[T], v T) (*Transmission, error) {
	b, tx, err := c.Encode(v, nil)
	if err != nil {
		return nil, err
	}
	if len(b) > 0 {
		if err := w.WriteFrame(nil, b); err != nil {
			return nil, err
		}
	}
	if err := w.WriteFrame(nil, nil); err != nil {
		return nil, err
	}
	return &Transmission{w: w, tx: tx}, nil
}

// Transmission transmits the asynchronous values pending in a transmitted value
type Transmission struct {
	w  *FrameWriter
	tx Transmitter
}

// Wait transmits the pending values until all of their paths are closed
func (t *Transmission) Wait(ctx context.Context) error {
	if t.tx == nil {
		return nil
	}
	return t.tx.Transmit(ctx, t.w, nil)
}
