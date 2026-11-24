//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	x := make(chan string)
	go func() {
		x <- "hello"
		close(x)
	}()
	rx2, inv, err := i.Ping(ctx, c, x, "world")
	if err != nil {
		return err
	}
	done := make(chan error, 1)
	go func() { done <- inv.Wait() }()
	m2 := <-rx2
	if err := <-done; err != nil {
		return err
	}
	assertEqual(m2, "helloworld")

	y := make(chan string)
	go func() {
		y <- m2
		close(y)
	}()
	m3, inv, err := i.Pong(ctx, c, y)
	if err != nil {
		return err
	}
	assertEqual(m3, "helloworld")
	if err := inv.Wait(); err != nil {
		return err
	}
	return nil
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
