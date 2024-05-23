package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"log"
	"log/slog"
	"os"

	"github.com/nats-io/nats.go"
	wasitypes "github.com/wrpc/wrpc/examples/go/http-outgoing-client/bindings/wasi/http/types"
	outgoing_handler "github.com/wrpc/wrpc/examples/go/http-outgoing-client/bindings/wrpc/http/outgoing_handler"
	wrpctypes "github.com/wrpc/wrpc/examples/go/http-outgoing-client/bindings/wrpc/http/types"
	wrpc "github.com/wrpc/wrpc/go"
	wrpcnats "github.com/wrpc/wrpc/go/nats"
)

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

	client := wrpcnats.NewClient(nc, "go")
	authority := "google.com"
	res, stop, err := outgoing_handler.Handle(context.Background(), client, &wrpctypes.Request{
		Body:      wrpc.NewCompleteReader(bytes.NewBuffer(nil)),
		Trailers:  wrpc.NewCompleteReceiver([]*wrpc.Tuple2[string, [][]uint8](nil)),
		Method:    wasitypes.NewMethodGet(),
		Scheme:    wasitypes.NewSchemeHttps(),
		Authority: &authority,
	}, &wrpctypes.RequestOptions{})
	if err != nil {
		return fmt.Errorf("failed to invoke `wrpc:http/outgoing-handler.handle`: %w", err)
	}
	if res.Err != nil {
		return fmt.Errorf("`wrpc:http/outgoing-handler.handle` invocation failed: %#v", *res.Err)
	}
	response := res.Ok
	fmt.Println(response.Status)
	for _, headers := range response.Headers {
		name := headers.V0
		values := headers.V1
		for _, v := range values {
			fmt.Println(name, string(v))
		}
	}
	body, err := io.ReadAll(response.Body)
	if err != nil {
		return fmt.Errorf("failed to read body: %w", err)
	}
	fmt.Println(string(body))
	if err := stop(); err != nil {
		return fmt.Errorf("failed to shutdown invocation")
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
