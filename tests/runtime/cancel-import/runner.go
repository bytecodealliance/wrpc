//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"
	"runtime"

	wrpc "wrpc.io/go"

	"driver/runner/my/test/i"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	pendingImport := func(ctx context.Context) (chan struct{}, <-chan error) {
		x := make(chan struct{})
		done := make(chan error, 1)
		go func() {
			errs, err := i.PendingImport(ctx, c, x)
			if errs != nil {
				for e := range errs {
					err = e
				}
			}
			done <- err
		}()
		return x, done
	}
	assertPending := func(done <-chan error) {
		runtime.Gosched()
		select {
		case <-done:
			panic("import completed before its future resolved")
		default:
		}
	}
	assertDropped := func(x chan<- struct{}) {
		select {
		case x <- struct{}{}:
			panic("future was not dropped")
		default:
		}
	}

	{
		ctx, cancel := context.WithCancel(ctx)
		x, done := pendingImport(ctx)
		assertPending(done)
		cancel()
		<-done
		assertDropped(x)
	}

	{
		ctx, cancel := context.WithCancel(ctx)
		cancel()
		x, done := pendingImport(ctx)
		<-done
		assertDropped(x)
	}

	{
		startedCtx, cancelStarted := context.WithCancel(ctx)
		x1, started := pendingImport(startedCtx)
		assertPending(started)

		if err := i.BackpressureSet(ctx, c, true); err != nil {
			return err
		}
		startingCtx, cancelStarting := context.WithCancel(ctx)
		x2, starting := pendingImport(startingCtx)
		assertPending(starting)

		cancelStarting()
		cancelStarted()

		if err := i.BackpressureSet(ctx, c, false); err != nil {
			return err
		}

		<-started
		<-starting
		assertDropped(x1)
		assertDropped(x2)
	}

	{
		ctx, cancel := context.WithCancel(ctx)
		x, done := pendingImport(ctx)
		assertPending(done)

		x <- struct{}{}
		close(x)

		for range 5 {
			runtime.Gosched()
		}

		cancel()
		<-done
	}

	{
		x1, started := pendingImport(ctx)
		assertPending(started)

		if err := i.BackpressureSet(ctx, c, true); err != nil {
			return err
		}
		startingCtx, cancelStarting := context.WithCancel(ctx)
		x2, starting := pendingImport(startingCtx)
		assertPending(starting)

		if err := i.BackpressureSet(ctx, c, false); err != nil {
			return err
		}
		for range 5 {
			runtime.Gosched()
		}

		cancelStarting()
		<-starting
		assertDropped(x2)

		x1 <- struct{}{}
		close(x1)
		if err := <-started; err != nil {
			return err
		}
	}
	return nil
}
