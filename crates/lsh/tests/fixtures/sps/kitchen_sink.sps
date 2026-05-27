# expect: 55
# sum 1..10 = 55. uses two locals: sum, i.
.fn main arity=0 locals=2
  # sum = 0
  PUSH_INT 0
  STORE_LOCAL 0
  # i = 1
  PUSH_INT 1
  STORE_LOCAL 1
loop:
  # if i > 10 goto end
  LOAD_LOCAL 1
  PUSH_INT 10
  GT
  JUMP_IF_FALSE body
  JUMP end
body:
  # sum = sum + i
  LOAD_LOCAL 0
  LOAD_LOCAL 1
  ADD_INT
  STORE_LOCAL 0
  # i = i + 1
  LOAD_LOCAL 1
  PUSH_INT 1
  ADD_INT
  STORE_LOCAL 1
  JUMP loop
end:
  LOAD_LOCAL 0
  RETURN

.entry main
