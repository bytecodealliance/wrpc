package outgoing_handler

import (
	"context"
	"errors"
	"fmt"
	"log/slog"

	wrpc "github.com/wrpc/wrpc/go"
	"github.com/wrpc/wrpc/go/interface/http/types"
)

func ServeHandle(c wrpc.Client, f func(context.Context, *types.RequestRecord, *types.RequestOptionsRecord) (*wrpc.Result[types.ResponseRecord, types.ErrorCodeVariant], error)) (func() error, error) {
	request := wrpc.NewSubscribePath().Index(0)
	body := request.Index(0)
	trailers := request.Index(1)
	return c.ServeIndex("wrpc:http/outgoing-handler@0.1.0", "handle", func(ctx context.Context, w wrpc.IndexWriter, r wrpc.IndexReader, errCh <-chan error) error {
		slog.DebugContext(ctx, "receiving `wrpc:http/types.request`")
		request, err := types.ReadRequest(r, 0)
		if err != nil {
			return fmt.Errorf("failed to receive `wrpc:http/types.request`: %w", err)
		}
		slog.DebugContext(ctx, "receiving `wrpc:http/types.request-options`")
		requestOptions, err := wrpc.ReadFlatOption(r, types.ReadRequestOptions)
		if err != nil {
			return fmt.Errorf("failed to receive `wrpc:http/types.request-options`: %w", err)
		}

		slog.DebugContext(ctx, "calling handler")
		res, err := f(ctx, request, requestOptions)
		if err != nil {
			return fmt.Errorf("handler failed: %w", err)
		}
		if err := func(v *wrpc.Result[types.ResponseRecord, types.ErrorCodeVariant], path ...uint32) error {
			switch {
			case v.Ok == nil && v.Err == nil:
				return errors.New("both result variants cannot be nil")
			case v.Ok != nil && v.Err != nil:
				return errors.New("exactly one result variant must non-nil")
			case v.Ok != nil:
				slog.Debug("writing `result::ok` status byte")
				if err := w.WriteByte(0); err != nil {
					return fmt.Errorf("failed to write `result::ok` status byte: %w", err)
				}
				slog.Debug("writing `result::ok` payload")
				f, err := v.Ok.WriteToIndex(w)
				if err != nil {
					return fmt.Errorf("failed to write `result::ok` payload: %w", err)
				}
				if f != nil {
					w, err = w.Index(0)
					if err != nil {
						return fmt.Errorf("failed to index writer: %w", err)
					}
					if err := f(w); err != nil {
						return fmt.Errorf("failed to write asynchronous `result::ok` payload: %w", err)
					}
				}
				return nil
			default:
				slog.Debug("writing `result::err` status byte")
				if err := w.WriteByte(1); err != nil {
					return fmt.Errorf("failed to write `result::err` status byte: %w", err)
				}
				slog.Debug("writing `result::err` payload")
				f, err := v.Err.WriteToIndex(w)
				if err != nil {
					return fmt.Errorf("failed to write `result::err` payload: %w", err)
				}
				if f != nil {
					w, err = w.Index(1)
					if err != nil {
						return fmt.Errorf("failed to index writer: %w", err)
					}
					if err := f(w); err != nil {
						return fmt.Errorf("failed to write asynchronous `result::err` payload: %w", err)
					}
				}
				return nil
			}
		}(res, 0); err != nil {
			return fmt.Errorf("failed to write result: %w", err)
		}
		return nil
	}, body, trailers)
}

func InvokeHandle(c wrpc.Client, ctx context.Context, request *types.RequestRecord, options *types.RequestOptionsRecord) (*wrpc.Result[types.ResponseRecord, types.ErrorCodeVariant], error) {
	panic("TODO")
}
