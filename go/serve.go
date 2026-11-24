package wrpc

import "context"

// Serve serves an invocation accepted by a Listener over `c` by calling `f` with its parameters, decoded using `pc`,
// and transmitting the results `f` returns, encoded using `rc`.
// The asynchronous parameters are received while `f` runs and the results are transmitted, it returns once all I/O is done, so they must be consumed.
// Any failure, including `f` failing, closes the byte streams of `c` and cancels the context of `f`, which must return then,
// since the values pending in the parameters stay pending. The error is a *ServeError unless `ctx` was cancelled.
func Serve[P, R any](ctx context.Context, c *Framed, pc Codec[P], rc Codec[R], f func(context.Context, P) (R, error)) error {
	ctx, cancel := context.WithCancelCause(ctx)
	defer cancel(nil)
	stop := context.AfterFunc(ctx, c.close)
	params, rx, err := Receive(c.R, pc)
	if err != nil {
		cancel(&ServeError{FaultRequest, err})
		return context.Cause(ctx)
	}
	rxDone := make(chan struct{})
	go func() {
		defer close(rxDone)
		if err := rx.Wait(ctx); err != nil {
			cancel(&ServeError{FaultRequest, err})
		}
	}()
	if err := serveResults(ctx, c.W, rc, f, params); err != nil {
		cancel(err)
	}
	<-rxDone
	if !stop() {
		return context.Cause(ctx)
	}
	return nil
}

func serveResults[P, R any](ctx context.Context, w *FrameWriter, rc Codec[R], f func(context.Context, P) (R, error), params P) error {
	results, err := f(ctx, params)
	if err != nil {
		return &ServeError{FaultHandler, err}
	}
	tx, err := Transmit(w, rc, results)
	if err != nil {
		return &ServeError{FaultResponse, err}
	}
	if err := tx.Wait(ctx); err != nil {
		return &ServeError{FaultResponse, err}
	}
	return nil
}
