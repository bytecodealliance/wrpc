// NOTE: This example is a work in-progress and will change significantly going forward

package main

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"

	"github.com/nats-io/nats.go"
	wrpc "github.com/wrpc/wrpc/go"
	"github.com/wrpc/wrpc/go/interface/http/outgoing_handler"
	"github.com/wrpc/wrpc/go/interface/http/types"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
)

func run() error {
	nc, err := nats.Connect(nats.DefaultURL)
	if err != nil {
		return fmt.Errorf("failed to connect to NATS.io: %w", err)
	}
	defer nc.Close()

	stop, err := outgoing_handler.ServeHandle(wrpcnats.NewClient(nc, "go"), func(ctx context.Context, request *types.RecordRequest, opts *types.RecordRequestOptions) (*types.RecordResponse, error) {
		scheme := request.Scheme.String()

		authority := ""
		if request.Authority != nil {
			authority = *request.Authority
		}

		pathWithQuery := ""
		if request.PathWithQuery != nil {
			pathWithQuery = *request.PathWithQuery
		}

		switch request.Method.Discriminant() {
		case types.DiscriminantMethod_Get:
			resp, err := http.Get(fmt.Sprintf("%s://%s/%s", scheme, authority, pathWithQuery))
			if err != nil {
				return nil, fmt.Errorf("request failed: %w", err)
			}
			headers := make([]*wrpc.Tuple2[string, [][]byte], 0, len(resp.Header))
			for header, values := range resp.Header {
				bs := make([][]byte, len(values))
				for i, value := range values {
					bs[i] = []byte(value)
				}
				headers = append(headers, &wrpc.Tuple2[string, [][]byte]{V0: header, V1: bs})
			}
			return &types.RecordResponse{
				Body:     wrpc.NewPendingByteReader(bufio.NewReader(resp.Body)),
				Trailers: nil, // Trailers wrpc.ReadyReceiver[[]*wrpc.Tuple2[string, [][]byte]]
				Status:   uint16(resp.StatusCode),
				Headers:  headers,
			}, nil
		default:
			return nil, errors.New("only GET currently supported")
		}
	})
	if err != nil {
		return err
	}

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, syscall.SIGINT)
	<-signalCh
	if err = stop(); err != nil {
		slog.Error("failed to stop serving", "err", err)
	}
	if err = nc.Drain(); err != nil {
		slog.Error("failed to drain NATS.io connection", "err", err)
	}
	return nil
}

func init() {
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		AddSource: true, Level: slog.LevelDebug, ReplaceAttr: func(groups []string, a slog.Attr) slog.Attr {
			if a.Key == slog.TimeKey {
				return slog.Attr{}
			}
			return a
		},
	})))
}

func main() {
	if err := run(); err != nil {
		slog.Error("failed to run", "err", err)
		os.Exit(1)
	}
}
