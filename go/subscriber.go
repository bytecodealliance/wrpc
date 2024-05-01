package wrpc

import (
	"context"
)

type Subscriber interface {
	Subscribe(func(context.Context, []byte), ...uint32) (func() error, error)
}

type NestedSubscriber struct {
	Subscriber
	Path []uint32
}

func (sub *NestedSubscriber) Subscribe(f func(context.Context, []byte), path ...uint32) (func() error, error) {
	return sub.Subscriber.Subscribe(f, append(sub.Path, path...)...)
}

func NewNestedSubscriber(sub Subscriber, path ...uint32) *NestedSubscriber {
	return &NestedSubscriber{sub, path}
}
