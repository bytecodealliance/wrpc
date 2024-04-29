package wrpc

import (
	"context"
)

type NestedSubscriber struct {
	Subscriber
	Path []uint32
}

func (sub *NestedSubscriber) Subscribe(f func(context.Context, []byte)) (func() error, error) {
	n := len(sub.Path)
	if n == 0 {
		return sub.Subscriber.Subscribe(f)
	}
	return sub.Subscriber.SubscribePath(sub.Path, f)
}

func (sub *NestedSubscriber) SubscribePath(path []uint32, f func(context.Context, []byte)) (func() error, error) {
	return sub.Subscriber.SubscribePath(append(sub.Path, path...), f)
}

func NewNestedSubscriber(sub Subscriber, path ...uint32) *NestedSubscriber {
	return &NestedSubscriber{sub, path}
}
