package wrpc

import "context"

type Transmitter interface {
	Transmit(context.Context, []uint32, []byte) error
}

type Subscriber interface {
	SubscribeBody(func(context.Context, []byte)) (func() error, error)
	SubscribeBodyElement([]*uint32, chan<- error, func(context.Context, []uint32, []byte)) (func() error, error)
	SubscribeError(func(context.Context, []byte)) (func() error, error)
}

type IncomingInvocation interface {
	Subscriber

	Accept(context.Context, []byte) error
}

type OutgoingInvocation interface {
	Subscriber

	Invoke(context.Context, []byte, func(context.Context, []byte)) (func() error, Transmitter, error)
}

type Client interface {
	NewInvocation(instance string, name string) OutgoingInvocation

	Serve(instance string, name string, f func(context.Context, []byte, Transmitter, IncomingInvocation) error) (func() error, error)
}
