- Host subscribes on a set of topics per exported component function. These will be referred to as invocation endpoints.
  Example:
    Component exports:
    - `mycompany:mypkg/myinterface@0.2.0-rc-2023-11-10.myfunc` freestanding function
    - `mycompany:mypkg/myinterface@0.2.0-rc-2023-11-10.myresource.mystaticfunc` static function on `mycompany:mypkg/myinterface@0.2.0-rc-2023-11-10.myresource` resource type
    - `mycompany:mypkg/myinterface@0.2.0-rc-2023-11-10.myresource.mymethod` method on `mycompany:mypkg/myinterface@0.2.0-rc-2023-11-10.myresource` resource type

    Host running component queue-subscribes on:
    - `[$prefix.]?wrpc.$ver.mycompany:mypkg/myinterface@0.2.0-rc-2023-11-10.myfunc`
    - `[$prefix.]?wrpc.$ver.mycompany:mypkg/myinterface@0.2.0-rc-2023-11-10.myresource!mystaticfunc`
    Where:
    - `$prefix` is optional and defined out-of-band, this could be e.g. a link group name
    - `$ver` is wRPC protocol version, e.g. `0.1.2`

    For each method `$method` on a guest-defined resource returned by the component, host listens on:
    - `$inbox.$method`
    Where
    - `$inbox` is a unique address in the NATS namespace, e.g. a NATS inbox, on which queue subscribe is never used

- Once a message with optional "reply" subject is received on an invocation endpoint, the receiving party (host, usually) must respond with optional "reply" subject
    - If "reply" is set in response to the invocation request, a two-way communication session is created, during which both parties may exchange messages concurrently and in any order

- The invocation request payload is always finite - it's size is known upfront my the invoker, any async values (e.g. streams, pollables and otherwise) are sent later async as part of the session
    - The invocation request payload may not fit in the configured NATS message size limit, in that case it's truncated and the rest of it is sent as part of the session

- As part of the session, parties send async values using an index-based path acquired through type reflection
  Example:
    - If second parameter in a function is an async value, e.g. a stream, it's contents would be sent on: `$inbox.1`
    - If third parameter in a function is a struct and it's first field is an async value, e.g. a stream, it's contents would be sent on: `$inbox.2.0`
