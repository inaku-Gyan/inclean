#include <string.h>

#include "cmsis_os2.h"                  // ::CMSIS:RTOS2
#include "../../../../../Drivers/CMSIS/Include/cmsis_compiler.h"             // Compiler agnostic definitions

#include "../include/FreeRTOS.h"                   // ARM.FreeRTOS::RTOS:Core
#include "../include/task.h"                       // ARM.FreeRTOS::RTOS:Core
#include "event_groups.h"               // ARM.FreeRTOS::RTOS:Event Groups
#include "../include/semphr.h"                     // ARM.FreeRTOS::RTOS:Core

#include "freertos_mpool.h"             // osMemoryPool definitions
#include "freertos_os2.h"               // Configuration check and setup
