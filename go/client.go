package wrpc

import (
	"context"
	"io"
)

type ErrorSubscriber interface {
	SubscribeError(func(context.Context, []byte)) (func() error, error)
}

type IncomingInvocation interface {
	Subscriber
	ErrorSubscriber

	Accept(context.Context, []byte) error
}

type SubscribePath []*uint32

func NewSubscribePath(ps ...*uint32) SubscribePath {
	return SubscribePath(ps)
}

func (p SubscribePath) push(v *uint32) SubscribePath {
	return SubscribePath(append(append(make(SubscribePath, 0, len(p)+1), p...), v))
}

func (p SubscribePath) Index(i uint32) SubscribePath {
	return p.push(&i)
}

func (p SubscribePath) Wildcard() SubscribePath {
	return p.push(nil)
}

func (p SubscribePath) Parent() (SubscribePath, bool) {
	n := len(p)
	if n == 0 {
		return nil, false
	}
	return SubscribePath(p[:n-1]), true
}

type Index[T any] interface {
	Index(path ...uint32) (T, error)
}

type IndexReader interface {
	io.Reader
	io.ByteReader

	Index[IndexReader]
}

type ReaderFromIndex interface {
	ReadFromIndex(IndexReader) error
}

type IndexWriter interface {
	io.Writer
	io.ByteWriter

	Index[IndexWriter]
}

type WriterToIndex interface {
	WriteToIndex(IndexWriter) error
}

type Client interface {
	Invoke(ctx context.Context, instance string, name string, f func(IndexWriter, IndexReader, <-chan error) error, subs ...SubscribePath) error
	ServeIndex(instance string, name string, f func(context.Context, IndexWriter, IndexReader, <-chan error) error, subs ...SubscribePath) (func() error, error)

	Serve(instance string, name string, f func(context.Context, []byte, Transmitter, IncomingInvocation) error) (func() error, error)
}
