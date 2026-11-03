//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"
	"runtime"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	pendingImport := func(ctx context.Context, x <-chan struct{}) <-chan error {
		done := make(chan error, 1)
		go func() {
			_, err := i.PendingImport(ctx, c, x)
			done <- err
		}()
		return done
	}
	assertPending := func(done <-chan error) {
		runtime.Gosched()
		select {
		case <-done:
			panic("import completed before its future resolved")
		default:
		}
	}

	{
		x := make(chan struct{}, 1)
		done := pendingImport(ctx, x)
		assertPending(done)
		assertPending(done)
		x <- struct{}{}
		close(x)
		if err := <-done; err != nil {
			return err
		}
	}

	// Start the imported function call, get it pending, then let it complete by
	// resolving `x`, then yield a few times to ensure that the runtime gets
	// the completion of the task-at-hand, and then cancel it without awaiting
	// it.
	{
		ctx, cancel := context.WithCancel(ctx)
		x := make(chan struct{}, 1)
		done := pendingImport(ctx, x)
		assertPending(done)
		x <- struct{}{}
		close(x)
		for range 5 {
			runtime.Gosched()
		}
		cancel()
	}
	return nil
}
