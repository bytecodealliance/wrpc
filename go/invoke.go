package wrpc

import "context"

// Invocation transmits the asynchronous parameters and receives the asynchronous results of an invocation
type Invocation struct {
	ctx    context.Context
	cancel context.CancelCauseFunc
	stop   func() bool
	tx     <-chan struct{}
	rx     *Receipt
}

// Invoke invokes function `name` of instance `instance` over `c`, encoding `params` with `pc` and decoding the results with `rc`.
// It returns the results once they are received, along with the Invocation, which must be waited for.
// Any failure, including cancellation of `ctx` before the Invocation completed, closes the byte streams of `c`.
// The error is an *InvokeError unless `ctx` was cancelled.
func Invoke[P, R any](ctx context.Context, c *Framed, instance, name string, pc Codec[P], rc Codec[R], params P) (R, *Invocation, error) {
	var zero R
	ctx, cancel := context.WithCancelCause(ctx)
	stop := context.AfterFunc(ctx, c.close)
	if err := c.W.WriteHeader(Header{Instance: instance, Name: name}); err != nil {
		cancel(&InvokeError{FaultRequest, err})
		return zero, nil, context.Cause(ctx)
	}
	tx, err := Transmit(c.W, pc, params)
	if err != nil {
		cancel(&InvokeError{FaultRequest, err})
		return zero, nil, context.Cause(ctx)
	}
	txDone := make(chan struct{})
	go func() {
		defer close(txDone)
		if err := tx.Wait(ctx); err != nil {
			cancel(&InvokeError{FaultRequest, err})
		}
	}()
	results, rx, err := Receive(c.R, rc)
	if err != nil {
		cancel(&InvokeError{FaultResponse, err})
		<-txDone
		return zero, nil, context.Cause(ctx)
	}
	return results, &Invocation{ctx: ctx, cancel: cancel, stop: stop, tx: txDone, rx: rx}, nil
}

// Wait transmits the asynchronous parameters and receives the asynchronous results until all of their paths are closed.
// The error is an *InvokeError unless the context of the invocation was cancelled.
func (inv *Invocation) Wait() error {
	if err := inv.rx.Wait(inv.ctx); err != nil {
		inv.cancel(&InvokeError{FaultResponse, err})
	}
	<-inv.tx
	if !inv.stop() {
		return context.Cause(inv.ctx)
	}
	inv.cancel(nil)
	return nil
}
