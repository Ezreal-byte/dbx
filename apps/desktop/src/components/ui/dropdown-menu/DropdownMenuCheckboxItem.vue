<script setup lang="ts">
import type { DropdownMenuCheckboxItemEmits, DropdownMenuCheckboxItemProps } from "reka-ui";

import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { CheckIcon } from "@lucide/vue";
import { DropdownMenuCheckboxItem, DropdownMenuItemIndicator, useForwardPropsEmits } from "reka-ui";
import { shouldSuppressRepeatedActivation, suppressEvent, type ActionActivationGuard } from "@/lib/connection/actionActivation";
import { cn } from "@/lib/common/utils";

const props = withDefaults(
  defineProps<
    DropdownMenuCheckboxItemProps & {
      class?: HTMLAttributes["class"];
      indicatorPosition?: "left" | "right";
    }
  >(),
  {
    indicatorPosition: "right",
  },
);
const emits = defineEmits<DropdownMenuCheckboxItemEmits>();

const delegatedProps = reactiveOmit(props, "class", "indicatorPosition");

const forwarded = useForwardPropsEmits(delegatedProps, emits);
const activationGuard: ActionActivationGuard = {};

function guardRepeatedClick(event: MouseEvent) {
  if (props.disabled) return;
  if (shouldSuppressRepeatedActivation(activationGuard)) {
    suppressEvent(event);
  }
}
</script>

<template>
  <DropdownMenuCheckboxItem
    data-slot="dropdown-menu-checkbox-item"
    v-bind="forwarded"
    @click.capture="guardRepeatedClick"
    :class="
      cn(
        'focus:bg-accent focus:text-accent-foreground focus:**:text-accent-foreground gap-1.5 rounded-md py-1 text-sm data-inset:pl-7 [&_svg:not([class*=size-])]:size-4 relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0',
        props.indicatorPosition === 'left' ? 'pr-2 pl-8' : 'pr-8 pl-1.5',
        props.class,
      )
    "
  >
    <span class="absolute flex h-4 w-4 items-center justify-center pointer-events-none" :class="props.indicatorPosition === 'left' ? 'left-2' : 'right-2'" data-slot="dropdown-menu-checkbox-item-indicator">
      <DropdownMenuItemIndicator>
        <slot name="indicator-icon">
          <CheckIcon />
        </slot>
      </DropdownMenuItemIndicator>
    </span>
    <slot />
  </DropdownMenuCheckboxItem>
</template>
