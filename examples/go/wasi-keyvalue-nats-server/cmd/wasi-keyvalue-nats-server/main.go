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

	"github.com/nats-io/nats.go"
	server "wrpc.io/examples/go/wasi-keyvalue-nats-server/bindings"
	"wrpc.io/examples/go/wasi-keyvalue-nats-server/bindings/exports/wasi/keyvalue/store"
	wrpc "wrpc.io/go"
	wrpcnats "wrpc.io/go/nats"
)

var _ store.Handler = &Handler{}

type Handler struct {
	data map[Bucket]map[string][]uint8
	lock sync.Mutex
}

type Bucket string

func (h *Handler) Open(ctx context.Context, identifier string) (*wrpc.Result[wrpc.Own[store.Bucket], store.Error], error) {
	h.lock.Lock()
	defer h.lock.Unlock()

	if h.data == nil {
		h.data = make(map[Bucket]map[string][]uint8)
	}
	bucket := (Bucket)(identifier)
	if _, ok := h.data[bucket]; !ok {
		h.data[bucket] = make(map[string][]uint8)
	}
	return wrpc.Ok[store.Error, wrpc.Own[store.Bucket]](wrpc.Own[store.Bucket](bucket)), nil
}

func (h *Handler) Bucket_Get(ctx__ context.Context, self wrpc.Borrow[store.Bucket], key string) (*wrpc.Result[[]uint8, store.Error], error) {
	h.lock.Lock()
	defer h.lock.Unlock()

	bucket := (Bucket)(self)
	if _, ok := h.data[bucket]; !ok {
		err := store.NewErrorNoSuchStore()
		return wrpc.Err[[]uint8, store.Error](*err), err
	}
	value, ok := h.data[bucket][key]
	if !ok {
		return wrpc.Ok[store.Error, []uint8](nil), nil
	}
	return wrpc.Ok[store.Error, []uint8](value), nil
}

func (h *Handler) Bucket_Set(ctx__ context.Context, self wrpc.Borrow[store.Bucket], key string, value []uint8) (*wrpc.Result[struct{}, store.Error], error) {
	h.lock.Lock()
	defer h.lock.Unlock()

	bucket := (Bucket)(self)
	if _, ok := h.data[bucket]; !ok {
		err := store.NewErrorNoSuchStore()
		return wrpc.Err[struct{}, store.Error](*err), err
	}
	h.data[bucket][key] = value
	return wrpc.Ok[store.Error, struct{}](struct{}{}), nil
}

func (h *Handler) Bucket_Delete(ctx__ context.Context, self wrpc.Borrow[store.Bucket], key string) (*wrpc.Result[struct{}, store.Error], error) {
	h.lock.Lock()
	defer h.lock.Unlock()

	bucket := (Bucket)(self)
	if _, ok := h.data[bucket]; !ok {
		err := store.NewErrorNoSuchStore()
		return wrpc.Err[struct{}, store.Error](*err), err
	}
	delete(h.data[bucket], key)

	return wrpc.Ok[store.Error, struct{}](struct{}{}), nil
}

func (h *Handler) Bucket_Exists(ctx__ context.Context, self wrpc.Borrow[store.Bucket], key string) (*wrpc.Result[bool, store.Error], error) {
	h.lock.Lock()
	defer h.lock.Unlock()

	bucket := (Bucket)(self)
	if _, ok := h.data[bucket]; !ok {
		err := store.NewErrorNoSuchStore()
		return wrpc.Err[bool, store.Error](*err), err
	}
	_, ok := h.data[bucket][key]
	return wrpc.Ok[store.Error, bool](ok), nil
}

func (h *Handler) Bucket_ListKeys(ctx__ context.Context, self wrpc.Borrow[store.Bucket], cursor *uint64) (*wrpc.Result[store.KeyResponse, store.Error], error) {
	h.lock.Lock()
	defer h.lock.Unlock()

	bucket := (Bucket)(self)
	if _, ok := h.data[bucket]; !ok {
		err := store.NewErrorNoSuchStore()
		return wrpc.Err[store.KeyResponse, store.Error](*err), err
	}
	keyResponse := store.KeyResponse{Keys: []string{}}
	for k := range h.data[bucket] {
		keyResponse.Keys = append(keyResponse.Keys, k)
	}
	return wrpc.Ok[store.Error, store.KeyResponse](keyResponse), nil
}

func run() error {
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

	client := wrpcnats.NewClient(nc, wrpcnats.WithPrefix("go"))
	stop, err := server.Serve(client, &Handler{})
	if err != nil {
		return fmt.Errorf("failed to serve `keyvalue` world: %w", err)
	}

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)
	<-signalCh

	if err = stop(); err != nil {
		return fmt.Errorf("failed to stop `keyvalue` world: %w", err)
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
