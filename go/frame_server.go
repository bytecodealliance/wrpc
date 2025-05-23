package wrpc

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"log/slog"
	"sync"
)

type handler struct {
	context.Context
	*sync.WaitGroup
	HandleFunc
	paths []SubscribePath
}

type FramedServer struct {
	ctx        context.Context
	handlersMu sync.RWMutex
	handlers   map[string]map[string]handler
}

func NewFramedServer(ctx context.Context) *FramedServer {
	return &FramedServer{
		ctx:      ctx,
		handlers: make(map[string]map[string]handler),
	}
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
	hs[name] = handler{
		Context:    ctx,
		WaitGroup:  &wg,
		HandleFunc: f,
		paths:      paths,
	}

	return func() error {
		cancel()

		s.handlersMu.Lock()
		defer s.handlersMu.Unlock()

		hs, ok := s.handlers[instance]
		if ok {
			delete(hs, name)
		}

		wg.Wait()

		return nil
	}, nil
}

func (s *FramedServer) Accept(w io.WriteCloser, r io.ReadCloser) error {
	br := bufio.NewReader(r)
	b, err := br.ReadByte()
	if err != nil {
		return fmt.Errorf("failed to read version byte: %w", err)
	}
	switch b {
	case 0x00:
		slog.DebugContext(s.ctx, "reading instance name string")
		instance, err := ReadString(br)
		if err != nil {
			return fmt.Errorf("failed to read instance name string: %w", err)
		}

		slog.DebugContext(s.ctx, "reading function name string")
		name, err := ReadString(br)
		if err != nil {
			return fmt.Errorf("failed to read function name string: %w", err)
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
