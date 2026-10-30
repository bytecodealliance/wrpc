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

// Accept reads the invocation header from `r` and calls the handler of the invoked function
// in a new goroutine. Reading the header is aborted, closing `w` and `r`, once `ctx` is done.
func (s *FramedServer) Accept(ctx context.Context, w io.WriteCloser, r io.ReadCloser) error {
	stop := context.AfterFunc(ctx, func() {
		if err := r.Close(); err != nil {
			slog.DebugContext(s.ctx, "failed to close reader", "err", err)
		}
	})
	if err := s.accept(w, r, func() error {
		if !stop() {
			return context.Cause(ctx)
		}
		return nil
	}); err != nil {
		stop()
		if cErr := w.Close(); cErr != nil {
			slog.DebugContext(s.ctx, "failed to close writer", "err", cErr)
		}
		if cErr := r.Close(); cErr != nil {
			slog.DebugContext(s.ctx, "failed to close reader", "err", cErr)
		}
		return err
	}
	return nil
}

func (s *FramedServer) accept(w io.WriteCloser, r io.ReadCloser, headerRead func() error) error {
	br := bufio.NewReader(r)
	b, err := br.ReadByte()
	if err != nil {
		return fmt.Errorf("failed to read version byte: %w", err)
	}
	switch b {
	case 0x00:
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
		if err := headerRead(); err != nil {
			return fmt.Errorf("failed to read invocation header: %w", err)
		}

		s.handlersMu.RLock()
		defer s.handlersMu.RUnlock()

		hs, ok := s.handlers[instance]
		if !ok {
			return fmt.Errorf("received an invocation for unknown instance `%s`", instance)
		}
		h, ok := hs[name]
		if !ok {
			return fmt.Errorf("received an invocation for unknown function `%s#%s`", instance, name)
		}

		h.Add(1)
		go func() {
			defer h.Add(-1)
			slog.DebugContext(h.Context, "calling handler", "instance", instance, "name", name)
			h.HandleFunc(
				h.Context,
				NewFrameStreamWriter(h.Context, w),
				NewFrameStreamReader(h.Context, BufReadCloser{
					Reader: br,
					Closer: r,
				}, h.paths...))
		}()
		return nil

	default:
		return fmt.Errorf("unsupported version byte: `0x%x`", b)
	}
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
