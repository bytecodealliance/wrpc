package wrpc

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"log/slog"
	"sync"
	"unicode/utf8"
)

const defaultMaxHeaderNameLen = 1 << 10

type handler struct {
	context.Context
	*sync.WaitGroup
	HandleFunc
	paths []SubscribePath
}

type FramedServer struct {
	ctx            context.Context
	handlersMu     sync.RWMutex
	handlers       map[string]map[string]handler
	maxInstanceLen uint32
	maxNameLen     uint32
}

// FramedServerOption configures a FramedServer
type FramedServerOption func(*FramedServer)

// WithMaxHeaderNameLen sets the maximum length in bytes of the instance and function names
// read from invocation headers, which are both 1 KiB by default
func WithMaxHeaderNameLen(instance, name uint32) FramedServerOption {
	return func(s *FramedServer) {
		s.maxInstanceLen = instance
		s.maxNameLen = name
	}
}

func NewFramedServer(ctx context.Context, opts ...FramedServerOption) *FramedServer {
	s := &FramedServer{
		ctx:            ctx,
		handlers:       make(map[string]map[string]handler),
		maxInstanceLen: defaultMaxHeaderNameLen,
		maxNameLen:     defaultMaxHeaderNameLen,
	}
	for _, opt := range opts {
		opt(s)
	}
	return s
}

func (s *FramedServer) Serve(instance string, name string, f HandleFunc, paths ...SubscribePath) (func() error, error) {
	ctx, cancel := context.WithCancel(s.ctx)
	var wg sync.WaitGroup

	s.handlersMu.Lock()
	defer s.handlersMu.Unlock()

	hs, ok := s.handlers[instance]
	if !ok {
		hs = make(map[string]handler)
		s.handlers[instance] = hs
	}
	if _, ok := hs[name]; ok {
		cancel()
		return nil, fmt.Errorf("handler for `%s#%s` already exists", instance, name)
	}
	hs[name] = handler{
		Context:    ctx,
		WaitGroup:  &wg,
		HandleFunc: f,
		paths:      paths,
	}

	return func() error {
		cancel()

		s.handlersMu.Lock()
		if hs, ok := s.handlers[instance]; ok && hs[name].WaitGroup == &wg {
			delete(hs, name)
		}
		s.handlersMu.Unlock()

		wg.Wait()

		return nil
	}, nil
}

// Accept serves the invocations on `w` and `r` one after another, until `r` ends in place of the
// next one, and then closes `w` and `r`. It reads each invocation header from `r`, which is
// aborted once `ctx` is done, calls the handler of the invoked function in a new goroutine and
// waits for the invocation to complete before reading the next one.
// If an invocation fails, `w` and `r` are closed with the error (see `CloseWithError`).
func (s *FramedServer) Accept(ctx context.Context, w io.WriteCloser, r io.ReadCloser) error {
	br := bufio.NewReader(r)
	for {
		err := s.accept(ctx, w, r, br)
		if err == io.EOF {
			if err := w.Close(); err != nil {
				slog.DebugContext(s.ctx, "failed to close writer", "err", err)
			}
			if err := r.Close(); err != nil {
				slog.DebugContext(s.ctx, "failed to close reader", "err", err)
			}
			return nil
		}
		if err != nil {
			if cErr := CloseWithError(r, err); cErr != nil {
				slog.DebugContext(s.ctx, "failed to close reader", "err", cErr)
			}
			if cErr := CloseWithError(w, err); cErr != nil {
				slog.DebugContext(s.ctx, "failed to close writer", "err", cErr)
			}
			return err
		}
	}
}

type invocationCloser struct {
	io.Closer
	once sync.Once
	done chan<- error
}

func (c *invocationCloser) Close() error {
	return c.CloseWithError(nil)
}

func (c *invocationCloser) CloseWithError(err error) (cErr error) {
	c.once.Do(func() {
		if err != nil {
			cErr = CloseWithError(c.Closer, err)
		}
		c.done <- err
	})
	return cErr
}

func (s *FramedServer) accept(ctx context.Context, w io.WriteCloser, r io.ReadCloser, br *bufio.Reader) error {
	stop := context.AfterFunc(ctx, func() {
		if err := CloseWithError(r, context.Cause(ctx)); err != nil {
			slog.DebugContext(s.ctx, "failed to close reader", "err", err)
		}
	})
	defer stop()

	b, err := br.ReadByte()
	if err == io.EOF && stop() {
		return io.EOF
	}
	if err != nil {
		return fmt.Errorf("failed to read version byte: %w", err)
	}
	if b != frameVersion {
		return fmt.Errorf("unsupported version byte: `0x%x`", b)
	}
	slog.DebugContext(s.ctx, "reading instance name string")
	instance, err := readHeaderName(br, s.maxInstanceLen)
	if err != nil {
		return fmt.Errorf("failed to read instance name string: %w", err)
	}

	slog.DebugContext(s.ctx, "reading function name string")
	name, err := readHeaderName(br, s.maxNameLen)
	if err != nil {
		return fmt.Errorf("failed to read function name string: %w", err)
	}
	if !stop() {
		return fmt.Errorf("failed to read invocation header: %w", context.Cause(ctx))
	}

	h, err := s.handler(instance, name)
	if err != nil {
		return err
	}
	done := make(chan error, 2)
	go func() {
		defer h.Add(-1)
		slog.DebugContext(h.Context, "calling handler", "instance", instance, "name", name)
		h.HandleFunc(
			h.Context,
			NewFrameStreamWriter(h.Context, struct {
				io.Writer
				*invocationCloser
			}{w, &invocationCloser{Closer: w, done: done}}),
			NewFrameStreamReader(h.Context, BufReadCloser{
				Reader: br,
				Closer: &invocationCloser{Closer: r, done: done},
			}, h.paths...))
	}()
	for range 2 {
		if err := <-done; err != nil {
			return err
		}
	}
	return h.Context.Err()
}

func (s *FramedServer) handler(instance, name string) (handler, error) {
	s.handlersMu.RLock()
	defer s.handlersMu.RUnlock()

	hs, ok := s.handlers[instance]
	if !ok {
		return handler{}, fmt.Errorf("received an invocation for unknown instance `%s`", instance)
	}
	h, ok := hs[name]
	if !ok {
		return handler{}, fmt.Errorf("received an invocation for unknown function `%s#%s`", instance, name)
	}
	h.Add(1)
	return h, nil
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
		return "", fmt.Errorf("name is not valid UTF-8")
	}
	return string(b), nil
}
