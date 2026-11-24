package wrpc

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"math"
	"net"
	"sync"
	"unicode/utf8"
)

const (
	frameVersion            = 0x01
	defaultMaxFrameDepth    = 64
	defaultMaxHeaderNameLen = 1 << 10
)

var (
	errDataAfterEnd  = errors.New("received data after value end")
	errClosedNoValue = fmt.Errorf("path closed without a value: %w", io.ErrUnexpectedEOF)
	errIngressEnded  = fmt.Errorf("ingress ended before all paths were closed: %w", io.ErrUnexpectedEOF)
)

func errUnknownPath(path []uint32) error {
	return fmt.Errorf("received frame on unknown path `%v`", path)
}

func unexpectedEOF(err error) error {
	switch {
	case err == io.EOF:
		return io.ErrUnexpectedEOF
	case errors.Is(err, io.EOF) && !errors.Is(err, io.ErrUnexpectedEOF):
		return fmt.Errorf("%w: %w", io.ErrUnexpectedEOF, err)
	default:
		return err
	}
}

// Header is the header of an invocation
type Header struct {
	Instance string
	Name     string
}

// FrameReader reads invocation headers and frames from a byte stream
type FrameReader struct {
	r    *bufio.Reader
	path []uint32
	n    uint32
}

// NewFrameReader returns a FrameReader reading from `r`
func NewFrameReader(r io.Reader) *FrameReader {
	return &FrameReader{r: bufio.NewReader(r)}
}

// ReadHeader reads the protocol version and the header of an invocation.
// It returns io.EOF if the byte stream ended before the header.
func (r *FrameReader) ReadHeader() (Header, error) {
	v, err := r.r.ReadByte()
	if err != nil {
		return Header{}, err
	}
	if v != frameVersion {
		return Header{}, fmt.Errorf("unsupported protocol version `0x%x`", v)
	}
	instance, err := readHeaderName(r.r, defaultMaxHeaderNameLen)
	if err != nil {
		return Header{}, fmt.Errorf("failed to read instance name: %w", unexpectedEOF(err))
	}
	name, err := readHeaderName(r.r, defaultMaxHeaderNameLen)
	if err != nil {
		return Header{}, fmt.Errorf("failed to read function name: %w", unexpectedEOF(err))
	}
	return Header{Instance: instance, Name: name}, nil
}

// Next reads the header of the next frame and returns its path, valid until the next call, and the length of its data.
// It returns io.EOF if the byte stream ended before the frame. The data of the previous frame must have been read.
func (r *FrameReader) Next() ([]uint32, uint32, error) {
	depth, err := ReadUint32(r.r)
	if err != nil {
		return nil, 0, err
	}
	if depth > defaultMaxFrameDepth {
		return nil, 0, fmt.Errorf("path length of %d exceeds maximum of %d", depth, defaultMaxFrameDepth)
	}
	r.path = r.path[:0]
	for range depth {
		i, err := ReadUint32(r.r)
		if err != nil {
			return nil, 0, unexpectedEOF(err)
		}
		r.path = append(r.path, i)
	}
	n, err := ReadUint32(r.r)
	if err != nil {
		return nil, 0, unexpectedEOF(err)
	}
	r.n = n
	return r.path, n, nil
}

// ReadByte reads a byte of the data of the current frame, returning io.EOF at its end
func (r *FrameReader) ReadByte() (byte, error) {
	if r.n == 0 {
		return 0, io.EOF
	}
	b, err := r.r.ReadByte()
	if err != nil {
		return 0, unexpectedEOF(err)
	}
	r.n--
	return b, nil
}

// Read reads the data of the current frame, returning io.EOF at its end
func (r *FrameReader) Read(p []byte) (int, error) {
	if r.n == 0 {
		return 0, io.EOF
	}
	if uint64(len(p)) > uint64(r.n) {
		p = p[:r.n]
	}
	n, err := r.r.Read(p)
	r.n -= uint32(n)
	if err != nil {
		return n, unexpectedEOF(err)
	}
	return n, nil
}

func (r *FrameReader) peek() ([]byte, error) {
	b, err := r.r.Peek(int(min(r.n, uint32(r.r.Size()))))
	if err != nil {
		return nil, unexpectedEOF(err)
	}
	return b, nil
}

func (r *FrameReader) discard(n int) {
	_, _ = r.r.Discard(n)
	r.n -= uint32(n)
}

// readHeaderName reads a name of at most `max` bytes from `r`
func readHeaderName(r *bufio.Reader, max uint32) (string, error) {
	n, err := ReadUint32(r)
	if err != nil {
		return "", fmt.Errorf("failed to read name length: %w", err)
	}
	if n > max {
		return "", fmt.Errorf("name length of %d exceeds maximum of %d", n, max)
	}
	b, err := ReadBytes(r, n)
	if err != nil {
		return "", fmt.Errorf("failed to read name: %w", err)
	}
	if !utf8.Valid(b) {
		return "", errors.New("name is not valid UTF-8")
	}
	return string(b), nil
}

// FrameWriter writes invocation headers and frames to a byte stream, it is safe for concurrent use
type FrameWriter struct {
	mu     sync.Mutex
	w      io.Writer
	header []byte
}

// NewFrameWriter returns a FrameWriter writing to `w`
func NewFrameWriter(w io.Writer) *FrameWriter {
	return &FrameWriter{w: w}
}

// WriteHeader writes the protocol version and the header `h` of an invocation
func (w *FrameWriter) WriteHeader(h Header) error {
	if !utf8.ValidString(h.Instance) || !utf8.ValidString(h.Name) {
		return errors.New("header is not valid UTF-8")
	}
	if uint64(len(h.Instance)) > math.MaxUint32 || uint64(len(h.Name)) > math.MaxUint32 {
		return errors.New("header name length overflows uint32")
	}
	w.mu.Lock()
	defer w.mu.Unlock()
	w.header = append(w.header[:0], frameVersion)
	w.header = AppendUint32(w.header, uint32(len(h.Instance)))
	w.header = append(w.header, h.Instance...)
	w.header = AppendUint32(w.header, uint32(len(h.Name)))
	w.header = append(w.header, h.Name...)
	_, err := w.w.Write(w.header)
	return err
}

// WriteFrame writes `data` as frames on `path`, an empty `data` closes `path`
func (w *FrameWriter) WriteFrame(path []uint32, data []byte) error {
	w.mu.Lock()
	defer w.mu.Unlock()
	for {
		n := min(uint64(len(data)), math.MaxUint32)
		w.header = AppendUint32(w.header[:0], uint32(len(path)))
		for _, i := range path {
			w.header = AppendUint32(w.header, i)
		}
		w.header = AppendUint32(w.header, uint32(n))
		bufs := net.Buffers{w.header}
		if n > 0 {
			bufs = append(bufs, data[:n])
		}
		if _, err := bufs.WriteTo(w.w); err != nil {
			return err
		}
		data = data[n:]
		if len(data) == 0 {
			return nil
		}
	}
}

// Framed is a byte stream pair carrying invocations one after another
//
// Any failure of an invocation closes both byte streams.
type Framed struct {
	W *FrameWriter
	R *FrameReader

	w io.WriteCloser
	r io.ReadCloser
}

// NewFramed returns a Framed writing to `w` and reading from `r`
func NewFramed(w io.WriteCloser, r io.ReadCloser) *Framed {
	return &Framed{W: NewFrameWriter(w), R: NewFrameReader(r), w: w, r: r}
}

func (c *Framed) close() {
	_ = c.w.Close()
	_ = c.r.Close()
}

// Listener accepts the invocations sent over a byte stream pair one after another
type Listener struct {
	c *Framed
}

// NewListener returns a Listener accepting the invocations read from `r`, whose results are written to `w`
func NewListener(w io.WriteCloser, r io.ReadCloser) *Listener {
	return &Listener{c: NewFramed(w, r)}
}

// Accept reads the header of the next invocation and returns it with the Framed to serve it over.
// It returns io.EOF once the byte stream ended in place of the next invocation.
// The invocation must be served before Accept is called again; Accept must not be called after a failure.
// Cancelling `ctx` while Accept waits closes the byte streams.
func (l *Listener) Accept(ctx context.Context) (Header, *Framed, error) {
	stop := context.AfterFunc(ctx, l.c.close)
	h, err := l.c.R.ReadHeader()
	if !stop() {
		return Header{}, nil, context.Cause(ctx)
	}
	if err == io.EOF {
		return Header{}, nil, err
	}
	if err != nil {
		l.c.close()
		return Header{}, nil, err
	}
	return h, l.c, nil
}
