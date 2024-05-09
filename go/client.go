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

type OutgoingInvocation interface {
	Subscriber
	ErrorSubscriber

	Invoke(context.Context, []byte, func(context.Context, []byte)) (func() error, Transmitter, error)
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
	NewInvocation(instance string, name string) OutgoingInvocation

	Invoke(instance string, name string, w WriterToIndex, subs ...[]*uint32) (IndexReader, <-chan error, error)
	ServeIndex(instance string, name string, f func(context.Context, IndexReader, <-chan error) (WriterToIndex, error), subs ...[]*uint32) (func() error, error)

	Serve(instance string, name string, f func(context.Context, []byte, Transmitter, IncomingInvocation) error) (func() error, error)
}
