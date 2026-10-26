//go:generate cargo run --bin wit-bindgen-wrpc go --generate-all --out-dir bindings --package wrpc.io/examples/go/streams-client/bindings wit

package app

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"sync"
	"time"

	"wrpc.io/examples/go/streams-client/bindings/wrpc_examples/streams/handler"
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

func throttle[T any](values []T, d time.Duration) <-chan []T {
	ch := make(chan []T)
	go func() {
		defer close(ch)
		tick := time.NewTicker(d)
		defer tick.Stop()
		for _, v := range values {
			<-tick.C
			ch <- []T{v}
		}
	}()
	return ch
}

func Run(prefix string, client wrpc.Invoker) error {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	numbers, bytes, errCh, err := handler.Echo(ctx, client, &handler.Req{
		Numbers: throttle([]uint64{1, 2, 3, 4, 5, 6, 7, 8, 9, 10}, time.Second),
		Bytes:   throttle([]byte("foo bar baz"), time.Second),
	})
	if err != nil {
		return fmt.Errorf("failed to call `wrpc-examples:streams/handler.echo`: %w", err)
	}
	var wg sync.WaitGroup
	defer wg.Wait()

	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-ctx.Done():
				return
			case chunk, ok := <-numbers:
				if !ok {
					return
				}
				fmt.Printf("%s numbers: %v\n", prefix, chunk)
			}
		}
	}()
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-ctx.Done():
				return
			case chunk, ok := <-bytes:
				if !ok {
					return
				}
				fmt.Printf("%s bytes: %s\n", prefix, chunk)
			}
		}
	}()
	var errs []error
	for err := range errCh {
		errs = append(errs, err)
		cancel()
	}
	if err := errors.Join(errs...); err != nil {
		return fmt.Errorf("failed to exchange async values: %w", err)
	}
	return nil
}
