//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	x := make(chan string, 1)
	x <- "hello"
	close(x)
	errs, err := i.TakeThenDrop(ctx, c, x)
	if err != nil {
		return err
	}
	for err := range errs {
		return err
	}

	x = make(chan string, 1)
	x <- "hello3"
	close(x)
	errs, err = i.ReadAndDrop(ctx, c, x)
	if err != nil {
		return err
	}
	for err := range errs {
		return err
	}
	return nil
}
