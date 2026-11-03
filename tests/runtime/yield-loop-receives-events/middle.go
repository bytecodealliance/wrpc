package middle

import (
	"context"
	"runtime"
	"sync/atomic"

	wrpc "wrpc.io/go"

	"driver/middle/test/common/i_middle"
)

var hit atomic.Bool

type handler struct {
	c wrpc.Invoker
}

func NewHandler(c wrpc.Invoker) handler {
	return handler{c}
}

func (h handler) F(ctx context.Context) error {
	task := make(chan error, 1)
	go func() {
		err := i_middle.F(ctx, h.c)
		if err == nil {
			hit.Store(true)
		}
		task <- err
	}()
	// This is an "infinite loop" but it's also effectively a yield which
	// should enable not only making progress on sibling goroutines but
	// additionally async events should be deliverable.
	for !hit.Load() && len(task) == 0 {
		runtime.Gosched()
	}
	return <-task
}
