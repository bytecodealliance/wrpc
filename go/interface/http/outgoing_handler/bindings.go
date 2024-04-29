package outgoing_handler

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"math"

	wrpc "github.com/wrpc/wrpc/go"
	"github.com/wrpc/wrpc/go/interface/http/types"
)

func ServeHandle(c wrpc.Client, f func(context.Context, *types.RecordRequest, *types.RecordRequestOptions) (*types.RecordResponse, error)) (func() error, error) {
	return c.Serve("wrpc:http/outgoing-handler@0.1.0", "handle", func(ctx context.Context, buffer []byte, tx wrpc.Transmitter, inv wrpc.IncomingInvocation) error {
		slog.DebugContext(ctx, "subscribe for `wrpc:http/outgoing-handler` parameters")

		payload := make(chan []byte)
		stop, err := inv.Subscribe(func(ctx context.Context, buf []byte) {
			payload <- buf
		})
		if err != nil {
			return err
		}
		defer func() {
			if err := stop(); err != nil {
				slog.ErrorContext(ctx, "failed to stop parameter subscription", "err", err)
			}
		}()

		slog.DebugContext(ctx, "subscribe for `wrpc:http/types.request`")
		requestSub, err := types.SubscribeRequest(wrpc.NewNestedSubscriber(inv, 0))
		if err != nil {
			return fmt.Errorf("failed to subscribe for `wrpc:http/types.request`: %w", err)
		}

		slog.DebugContext(ctx, "accept handshake")
		if err := inv.Accept(ctx, nil); err != nil {
			return fmt.Errorf("failed to complete handshake: %w", err)
		}

		r := wrpc.NewChanReader(ctx, payload, buffer)

		slog.DebugContext(ctx, "receive `wrpc:http/types.request`")
		request, err := types.ReadRequest(ctx, r, requestSub)
		if err != nil {
			return fmt.Errorf("failed to receive `wrpc:http/types.request`: %w", err)
		}
		slog.DebugContext(ctx, "receive `wrpc:http/types.request-options`")
		requestOptions, err := wrpc.ReadFlatOption(r, types.ReadRequestOptions)
		if err != nil {
			return fmt.Errorf("failed to receive `wrpc:http/types.request-options`: %w", err)
		}

		slog.DebugContext(ctx, "call `wrpc:http/outgoing-handler.handle` handler")
		resp, err := f(ctx, request, requestOptions)
		if err != nil {
			return fmt.Errorf("failed to handle `wrpc:http/outgoing-handler.handle`: %w", err)
		}

		respBody, err := io.ReadAll(resp.Body)
		if err != nil {
			return fmt.Errorf("failed to read body: %w", err)
		}
		res := []byte{0, 1}
		res = wrpc.AppendUint32(res, uint32(len(respBody)))
		res = append(res, respBody...)
		res = append(res, 1, 0) // empty trailers
		res = wrpc.AppendUint16(res, resp.Status)

		nh := len(resp.Headers)
		if nh > math.MaxUint32 {
			return fmt.Errorf("header count %d overflows uint32", nh)
		}
		res = wrpc.AppendUint32(res, uint32(nh))
		for _, header := range resp.Headers {
			nv := len(header.V1)
			if nv > math.MaxUint32 {
				return fmt.Errorf("header value count %d overflows uint32", nv)
			}
			res, err = wrpc.AppendString(res, header.V0)
			if err != nil {
				return fmt.Errorf("failed to encode header name: %w", err)
			}
			res = wrpc.AppendUint32(res, uint32(nv))
			for _, value := range header.V1 {
				res, err = wrpc.AppendString(res, string(value))
				if err != nil {
					return fmt.Errorf("failed to encode header value: %w", err)
				}
			}
		}
		if err := tx.Transmit(context.Background(), nil, res); err != nil {
			return fmt.Errorf("failed to transmit result: %w", err)
		}
		return nil
	})
}
