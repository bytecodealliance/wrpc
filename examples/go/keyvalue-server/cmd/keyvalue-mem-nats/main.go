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
	server "github.com/wrpc/wrpc/examples/go/keyvalue-server/bindings"
	"github.com/wrpc/wrpc/examples/go/keyvalue-server/bindings/exports/wrpc/keyvalue/store"
	wrpc "github.com/wrpc/wrpc/go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
)

var (
	errNoSuchStore     = store.NewError_NoSuchStore()
	errInvalidDataType = store.NewError_Other("invalid data type stored in map")
)

type Handler struct {
	sync.Map
}

func Ok[T any](v T) *wrpc.Result[T, store.Error] {
	return wrpc.Ok[store.Error](v)
}

func (h *Handler) Delete(ctx context.Context, bucket string, key string) (*wrpc.Result[struct{}, store.Error], error) {
	slog.InfoContext(ctx, "handling `wrpc:keyvalue/store.delete`", "bucket", bucket, "key", key)
	v, ok := h.Load(bucket)
	if !ok {
		return wrpc.Err[struct{}](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[struct{}](*errInvalidDataType), nil
	}
	b.Delete(key)
	return Ok(struct{}{}), nil
}

func (h *Handler) Exists(ctx context.Context, bucket string, key string) (*wrpc.Result[bool, store.Error], error) {
	slog.InfoContext(ctx, "handling `wrpc:keyvalue/store.exists`", "bucket", bucket, "key", key)
	v, ok := h.Load(bucket)
	if !ok {
		return wrpc.Err[bool](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[bool](*errInvalidDataType), nil
	}
	slog.InfoContext(ctx, "delete", "bucket", bucket, "key", key)
	_, ok = b.Load(key)
	return Ok(ok), nil
}

func (h *Handler) Get(ctx context.Context, bucket string, key string) (*wrpc.Result[[]uint8, store.Error], error) {
	slog.InfoContext(ctx, "handling `wrpc:keyvalue/store.get`", "bucket", bucket, "key", key)
	v, ok := h.Load(bucket)
	if !ok {
		return wrpc.Err[[]uint8](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[[]uint8](*errInvalidDataType), nil
	}
	v, ok = b.Load(key)
	if !ok {
		return Ok([]uint8(nil)), nil
	}
	buf, ok := v.([]byte)
	if !ok {
		return wrpc.Err[[]uint8](*errInvalidDataType), nil
	}
	return Ok(buf), nil
}

func (h *Handler) Set(ctx context.Context, bucket string, key string, value []byte) (*wrpc.Result[struct{}, store.Error], error) {
	slog.InfoContext(ctx, "handling `wrpc:keyvalue/store.set`", "bucket", bucket, "key", key, "value", value)
	b := &sync.Map{}
	v, ok := h.LoadOrStore(bucket, b)
	if ok {
		b, ok = v.(*sync.Map)
		if !ok {
			return wrpc.Err[struct{}](*errInvalidDataType), nil
		}
	}
	b.Store(key, value)
	return Ok(struct{}{}), nil
}

func (h *Handler) ListKeys(ctx context.Context, bucket string, cursor *uint64) (*wrpc.Result[store.KeyResponse, store.Error], error) {
	slog.InfoContext(ctx, "handling `wrpc:keyvalue/store.list-keys`", "bucket", bucket, "cursor", cursor)
	if cursor != nil {
		return wrpc.Err[store.KeyResponse](*store.NewError_Other("cursors are not supported")), nil
	}
	b := &sync.Map{}
	v, ok := h.LoadOrStore(bucket, b)
	if ok {
		b, ok = v.(*sync.Map)
		if !ok {
			return wrpc.Err[store.KeyResponse](*errInvalidDataType), nil
		}
	}
	var keys []string
	var err *store.Error
	b.Range(func(k, _ any) bool {
		s, ok := k.(string)
		if !ok {
			err = errInvalidDataType
			return false
		}
		keys = append(keys, s)
		return true
	})
	if err != nil {
		return wrpc.Err[store.KeyResponse](*err), nil
	}
	return Ok(store.KeyResponse{
		Keys:   keys,
		Cursor: nil,
	}), nil
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
	stop, err := server.Serve(wrpc, &Handler{})
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
