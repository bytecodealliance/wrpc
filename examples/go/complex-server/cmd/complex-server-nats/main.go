package main

import (
	"context"
	"fmt"
	"log"
	"log/slog"
	"os"
	"os/signal"
	"sync"
	"syscall"

	server "github.com/bytecodealliance/wrpc/examples/go/complex-server/bindings"
	"github.com/bytecodealliance/wrpc/examples/go/complex-server/bindings/exports/wrpc_examples/complex/resources"
	wrpc "github.com/bytecodealliance/wrpc/go"
	wrpcnats "github.com/bytecodealliance/wrpc/go/nats"
	"github.com/google/uuid"
	"github.com/nats-io/nats.go"
)

type Foo struct {
	cancel context.CancelFunc
	id     uuid.UUID
}

func (Foo) Bar(ctx context.Context) (string, error) {
	return "bar", nil
}

type ResourcesHandler struct {
	sync.Map
}

func (h *ResourcesHandler) Foo(ctx context.Context) (resources.HandlerFoo, context.Context, string, error) {
	id, err := uuid.NewV7()
	if err != nil {
		return nil, nil, "", fmt.Errorf("failed to generate UUIDv7: %w", err)
	}
	ctx, cancel := context.WithCancel(ctx)
	v := Foo{id: id, cancel: cancel}
	h.Store(id.String(), v)
	go func() {
		<-ctx.Done()
		h.Delete(id)
	}()
	return v, ctx, id.String(), nil
}

func (h *ResourcesHandler) Foo_Foo(ctx context.Context, v wrpc.Own[resources.Foo]) (string, error) {
	stored, ok := h.Load(string(v))
	if !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	foo := stored.(Foo)
	foo.cancel()
	return "foo", nil
}

func (h *ResourcesHandler) Bar(ctx context.Context, v wrpc.Borrow[resources.Foo]) (string, error) {
	stored, ok := h.Load(string(v))
	if !ok {
		return "", fmt.Errorf("unknown resource ID `%s`", string(v))
	}
	return stored.(Foo).Bar(ctx)
}

func run() (err error) {
	nc, err := nats.Connect(nats.DefaultURL)
	if err != nil {
		return fmt.Errorf("failed to connect to NATS.io: %w", err)
	}
	defer nc.Close()
	defer func() {
		if dErr := nc.Drain(); dErr != nil {
			if err == nil {
				err = fmt.Errorf("failed to drain NATS.io connection: %w", dErr)
			} else {
				slog.Error("failed to drain NATS.io connection", "err", dErr)
			}
		}
	}()

	wrpc := wrpcnats.NewClient(nc, "go")
	stop, err := server.Serve(wrpc, &ResourcesHandler{})
	if err != nil {
		return fmt.Errorf("failed to serve `server` world: %w", err)
	}

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)
	<-signalCh

	if err = stop(); err != nil {
		return fmt.Errorf("failed to stop `server` world: %w", err)
	}
	return nil
}

func init() {
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelInfo, ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}

func main() {
	if err := run(); err != nil {
		log.Fatal(err)
	}
}
