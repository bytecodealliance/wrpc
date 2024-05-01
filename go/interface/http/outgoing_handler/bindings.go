package outgoing_handler

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"sync"

	wrpc "github.com/wrpc/wrpc/go"
	"github.com/wrpc/wrpc/go/interface/http/types"
)

func ServeHandle(c wrpc.Client, f func(context.Context, *types.RequestRecord, *types.RequestOptionsRecord) (*wrpc.Result[types.ResponseRecord, types.ErrorCodeVariant], func(), error)) (func() error, error) {
	return c.Serve("wrpc:http/outgoing-handler@0.1.0", "handle", func(ctx context.Context, buffer []byte, tx wrpc.Transmitter, inv wrpc.IncomingInvocation) error {
		slog.DebugContext(ctx, "subscribing for `wrpc:http/outgoing-handler` parameters")

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

		slog.DebugContext(ctx, "subscribing for `wrpc:http/types.request`")
		requestSub, err := types.SubscribeRequest(wrpc.NewNestedSubscriber(inv, 0))
		if err != nil {
			return fmt.Errorf("failed to subscribe for `wrpc:http/types.request`: %w", err)
		}

		slog.DebugContext(ctx, "accepting handshake")
		if err := inv.Accept(ctx, nil); err != nil {
			return fmt.Errorf("failed to complete handshake: %w", err)
		}

		r := wrpc.NewChanReader(ctx, payload, buffer)

		slog.DebugContext(ctx, "receiving `wrpc:http/types.request`")
		request, err := types.ReadRequest(ctx, r, requestSub)
		if err != nil {
			return fmt.Errorf("failed to receive `wrpc:http/types.request`: %w", err)
		}
		slog.DebugContext(ctx, "receiving `wrpc:http/types.request-options`")
		requestOptions, err := wrpc.ReadFlatOption(r, types.ReadRequestOptions)
		if err != nil {
			return fmt.Errorf("failed to receive `wrpc:http/types.request-options`: %w", err)
		}

		slog.DebugContext(ctx, "calling handler")
		res, cleanup, err := f(ctx, request, requestOptions)
		if err != nil {
			return fmt.Errorf("handler failed: %w", err)
		}
		if cleanup != nil {
			defer cleanup()
		}

		var buf bytes.Buffer
		var okWriter *types.ResponseTransmitter
		slog.DebugContext(ctx, "encoding result")
		if err = res.WriteTo(&buf, func(resp *types.ResponseRecord, w wrpc.ByteWriter) error {
			var err error
			okWriter, err = resp.WriteTo(w)
			return err
		}, (*types.ErrorCodeVariant).WriteTo); err != nil {
			return fmt.Errorf("failed to encode handler result: %w", err)
		}
		var wg sync.WaitGroup
		var okErr error
		if okWriter != nil {
			wg.Add(1)
			go func() {
				defer wg.Done()
				slog.DebugContext(ctx, "transmitting async `result::ok`")
				okErr = okWriter.Transmit(ctx, wrpc.NewNestedTransmitter(tx, 0))
			}()
		}
		slog.DebugContext(ctx, "transmitting result")
		if err := tx.Transmit(context.Background(), buf.Bytes()); err != nil {
			return fmt.Errorf("failed to transmit result: %w", err)
		}
		slog.DebugContext(ctx, "waiting for async transmissions to finish")
		wg.Wait()
		if okErr != nil {
			return fmt.Errorf("failed to transmit async `result::ok`: %w", okErr)
		}
		return nil
	})
}
