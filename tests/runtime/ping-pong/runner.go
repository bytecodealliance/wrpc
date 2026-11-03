//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	x := make(chan string)
	go func() {
		x <- "hello"
		close(x)
	}()
	rx2, errs, err := i.Ping(ctx, c, x, "world")
	if err != nil {
		return err
	}
	m2 := <-rx2
	for err := range errs {
		return err
	}
	assertEqual(m2, "helloworld")

	x = make(chan string)
	go func() {
		x <- m2
		close(x)
	}()
	m3, errs, err := i.Pong(ctx, c, x)
	if err != nil {
		return err
	}
	assertEqual(m3, "helloworld")
	for err := range errs {
		return err
	}
	return nil
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
