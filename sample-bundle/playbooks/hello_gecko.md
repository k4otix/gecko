---
type: playbook
title: Hello GECKO
description: A sample playbook to verify the execution sandbox is working correctly.
engine: rhai
tags: [demo, hello-world]
---

# Hello GECKO

This is a sample playbook demonstrating that the OKF parser, the TypeDB 3.0 knowledge graph syncer, and the Rhai execution sandbox are all working in harmony!

Below is the code block that the `gecko run` command will extract from the knowledge graph and execute in the sandbox:

```rhai
// We can define variables and perform calculations
let framework = "GECKO";
let engine = "Rhai Sandbox";
let answer = 40 + 2;

print("Hello from " + framework + " running in the " + engine + "!");
print("The meaning of life, the universe, and everything is: " + answer);

// The final expression is returned as the script's structured output
"Execution Successful"
```
