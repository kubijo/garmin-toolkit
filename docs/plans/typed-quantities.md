# Typed quantities

Use typed quantities throughout code we control, including calculations, snapshots, digest inputs and formatters. Keep
the type through serialization when it can produce the required wire representation. Convert to a scalar at the call or
adapter for an external interface that requires one, such as FIT, SQL, OS calls or plotting APIs. Follow
`std::time::Duration` for quantity APIs. Use unit-named constructors and accessors, represent values exactly where
possible, and make overflow behavior explicit. Put unit-aware formatting on the quantity type.
