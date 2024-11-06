//go:generate cargo run --bin wit-bindgen-wrpc go --out-dir bindings --package wrpc.io/examples/go/wasi-keyvalue-server/bindings wit

package app

import (
	"context"
	"log/slog"
	"os"
	"sync"

	"wrpc.io/examples/go/wasi-keyvalue-server/bindings/exports/wasi/keyvalue/store"
	wrpc "wrpc.io/go"
)

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

var (
	errNoSuchStore     = store.NewErrorNoSuchStore()
	errInvalidDataType = store.NewErrorOther("invalid data type stored in map")
)

type Handler struct {
	sync.Map
}

func Ok[T any](v T) *wrpc.Result[T, store.Error] {
	return wrpc.Ok[store.Error](v)
}

func (h *Handler) Open(ctx context.Context, identifier string) (*wrpc.Result[wrpc.Own[store.Bucket], store.Error], error) {
	slog.InfoContext(ctx, "handling `wasi:keyvalue/store.open`", "identifier", identifier)
	h.LoadOrStore(string(identifier), &sync.Map{})
	return Ok(wrpc.Own[store.Bucket](identifier)), nil
}

func (h *Handler) Bucket_Get(ctx context.Context, bucket wrpc.Borrow[store.Bucket], key string) (*wrpc.Result[[]byte, store.Error], error) {
	slog.InfoContext(ctx, "handling `wasi:keyvalue/store.bucket.get`", "bucket", bucket, "key", key)
	v, ok := h.Load(string(bucket))
	if !ok {
		return wrpc.Err[[]byte](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[[]byte](*errInvalidDataType), nil
	}
	v, ok = b.Load(key)
	if !ok {
		return Ok([]byte(nil)), nil
	}
	buf, ok := v.([]byte)
	if !ok {
		return wrpc.Err[[]byte](*errInvalidDataType), nil
	}
	return Ok(buf), nil
}

func (h *Handler) Bucket_Set(ctx context.Context, bucket wrpc.Borrow[store.Bucket], key string, value []byte) (*wrpc.Result[struct{}, store.Error], error) {
	slog.InfoContext(ctx, "handling `wasi:keyvalue/store.bucket.set`", "bucket", bucket, "key", key, "value", value)
	v, ok := h.Load(string(bucket))
	if !ok {
		return wrpc.Err[struct{}](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[struct{}](*errInvalidDataType), nil
	}
	b.Store(key, value)
	return Ok(struct{}{}), nil
}

func (h *Handler) Bucket_Delete(ctx context.Context, bucket wrpc.Borrow[store.Bucket], key string) (*wrpc.Result[struct{}, store.Error], error) {
	slog.InfoContext(ctx, "handling `wasi:keyvalue/store.bucket.delete`", "bucket", bucket, "key", key)
	v, ok := h.Load(string(bucket))
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

func (h *Handler) Bucket_Exists(ctx context.Context, bucket wrpc.Borrow[store.Bucket], key string) (*wrpc.Result[bool, store.Error], error) {
	slog.InfoContext(ctx, "handling `wasi:keyvalue/store.bucket.exists`", "bucket", bucket, "key", key)
	v, ok := h.Load(string(bucket))
	if !ok {
		return wrpc.Err[bool](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[bool](*errInvalidDataType), nil
	}
	_, ok = b.Load(key)
	return Ok(ok), nil
}

func (h *Handler) Bucket_ListKeys(ctx context.Context, bucket wrpc.Borrow[store.Bucket], cursor *string) (*wrpc.Result[store.KeyResponse, store.Error], error) {
	slog.InfoContext(ctx, "handling `wasi:keyvalue/store.bucket.list-keys`", "bucket", bucket, "cursor", cursor)
	if cursor != nil {
		return wrpc.Err[store.KeyResponse](*store.NewErrorOther("cursors are not supported")), nil
	}
	v, ok := h.Load(string(bucket))
	if !ok {
		return wrpc.Err[store.KeyResponse](*errNoSuchStore), nil
	}
	b, ok := v.(*sync.Map)
	if !ok {
		return wrpc.Err[store.KeyResponse](*errInvalidDataType), nil
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
