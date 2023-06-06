#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

// dev_mode relax some restrictions placed on production pallets, such as no need to specify a
// weight on every `#[pallet::call]` Note: remove dev_mode before deploying in a production runtime.
#[frame_support::pallet(dev_mode)]
pub mod pallet {
	use frame_support::{
		pallet_prelude::*,
		sp_runtime::offchain::storage::StorageValueRef,
		sp_std,
		traits::{Currency, Hooks, Randomness},
	};
	use frame_system::pallet_prelude::*;
	use serde::{Deserialize, Serialize};
	use sp_core::hexdisplay;
	use sp_io::offchain_index;
	use sp_std::prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	// Defines generic data types that the pallet uses
	// By including the Currency and Randomness interfaces the pallet will be able to:
	// 	- access and manipulate user accounts and balances
	// 	- generate on-chain randomness
	// 	- set the limit on the number of newsletters a single user can subscribe to
	// frame_system|frame_support types: AccountId, BlockNumber, Hash
	#[pallet::config]
	pub trait Config: frame_system::Config {
		// the type used accross the runtime for emitting events
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		// the interface used to manage user balances
		type Currency: Currency<Self::AccountId>;

		// the interface used to get random values
		// mapping of a feed hash to the block number
		type NewsFeedRandomness: Randomness<Self::Hash, Self::BlockNumber>;

		// max number of created feeds. Each feed may have multiple subscriptions.
		#[pallet::constant]
		type MaxNewsFeeds: Get<u32>;

		// max number of owned subscriptions
		#[pallet::constant]
		type MaxOwnedNewsFeeds: Get<u32>;

		// max number of subscriptions per account
		#[pallet::constant]
		type MaxOwnedSubscriptions: Get<u32>;

		// max length of the subscription registration url
		#[pallet::constant]
		type MaxRegistrationUrlLength: Get<u32>;

		// max number of subscriptions waiting for registration for the current block
		#[pallet::constant]
		type MaxSubscriptionsPerBlock: Get<u32>;
	}

	// Payment type that can be either one time or recurrent
	#[derive(Clone, Copy, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	pub enum PaymentType {
		OneTime,
	}

	// Allows easy access to the pallet's Balance type.
	type BalanceOf<T> =
		<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

	// Unique id of objects created by this pallet, e.g. news feed, subscription
	type UniqueId = [u8; 16];

	// Encode - serialization of a custom type to bytes
	// Decode - deserialization of bytes into a custom type
	// TypeInfo - information of a custom type to the runtime metodata
	// MaxEncodedLen - ensures that types in the runtime are bounded in size
	// RuntimeDebug - allow a type to be printed to the console

	// TypeInfo macro forces to parse the underlying object into some JSON type. The Newseed type is
	// generic over T, we don't want to include it in the TypeInfo generation step. That's why
	// skip_type_params(T) is needed
	#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	pub struct NewsFeed<T: Config> {
		// unique identifier of the news feed
		pub id: UniqueId,
		// owner of the subscription
		pub owner: T::AccountId,
		// fee taken when an account subscribes to the news feed. None assumes subscription not allowed.
		pub subscribe_fee: Option<BalanceOf<T>>,
		// fee taken when an account unsubscribes from the news feed
		pub unsubscribe_fee: Option<BalanceOf<T>>,
		// payment type
		pub payment_type: PaymentType,
		// url of where to register/revoke a subscription
		// Vec<u8> represents a string in substrate
		// pub registration_url: Vec<u8>,
		pub registration_url: BoundedVec<u8, T::MaxRegistrationUrlLength>,
	}

	// Represents account subscription of the news feed
	#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	pub struct Subscription<T: Config> {
		// unique identifier of the news feed subscription
		pub id: UniqueId,
		// unique identifier of the subscribed news feed
		pub newsfeed_id: UniqueId,
		// subscription owner
		pub owner: T::AccountId,
	}

	// Stores single value that is a number of  available news feeds.
	// The value is incremented each time news feed is created.
	// ValueQuery - if there is no value, the zero value is returned,
	// OptionQuery - None is returned,
	// ResultQuery - Err is returned.
	#[pallet::storage]
	pub(super) type NewsFeedCount<T: Config> = StorageValue<_, u32, ValueQuery>;

	// Stores a map of feeds (each with unique_id) to their properties
	// The Twox64Concat is hashing algorithm used to store the map value.
	#[pallet::storage]
	pub(super) type NewsFeeds<T: Config> = StorageMap<_, Twox64Concat, UniqueId, NewsFeed<T>>;

	// Tracks who owns what news feeds
	#[pallet::storage]
	pub(super) type OwnerOfNewsFeeds<T: Config> = StorageMap<
		_,
		Twox64Concat,
		T::AccountId,
		BoundedVec<UniqueId, T::MaxOwnedNewsFeeds>,
		ValueQuery,
	>;

	// Tracks the news feeds subscribed by each account
	#[pallet::storage]
	pub(super) type Subscriptions<T: Config> = StorageMap<
		_,
		Twox64Concat,
		T::AccountId,
		BoundedVec<Subscription<T>, T::MaxOwnedSubscriptions>,
		ValueQuery,
	>;

	// Queues subscriptions waiting for registrations for the current block. Multiple transactions within one block register records in this queue
	// Block finalization removes then and sends them to the offchain index.
	#[pallet::storage]
	pub(super) type SubscriptionsPerBlock<T: Config> =
		StorageValue<_, BoundedVec<UniqueId, T::MaxSubscriptionsPerBlock>, ValueQuery>;

	// create_subscription creates new unique subscription
	// - create unique id
	// - ensure that total number of subscriptions does not exceed the maximum allowed

	#[pallet::error]
	pub enum Error<T> {
		// each feed must have a unique identifier
		DuplicatedNewsFeed,
		// an account is not an owner of the newsfeed
		NotNewsFeedOwner,
		// the total supply of feeds can't exceed `Config::MaxFeeds`
		NewsFeedsOverflow,
		// exceeded maximum of news feed assigned to an owner
		MaxNewsFeedsOwned,
		// news feed not found
		NewsFeedNotFound,
		// owner and target are the same
		TransferToSelf,
		// a feed url can't exceed `Config::MaxRegistrationUrlLength`
		RegistrationUrlTooLong,
		// an account must subscribe only once to the feed
		DuplicatedSubscription,
		// an account can't exceed `Config::MaxSubscriptions`
		MaxSubscriptionsExceeded,
		// an account can't subscribe to a given news feed
		SubscriptionNotAllowed,
		// subscription is not assigned to a given account
		SubscriptionNotFound,
		// Exceeded max number of subscriptions per block
		MaxSubscriptionsPerBlock,
	}

	// generate_deposit generates a helper function on Pallet that handles event depositing/sending
	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		// runtime sends FeedCreated event when new feed has been successfully created
		NewsFeedCreated {
			newsfeed: UniqueId,
			owner: T::AccountId,
		},
		// runtime sends NewsFeed transfered event when NewsFeed changed the ownership
		NewsFeedTransferred {
			newsfeed: UniqueId,
			from: T::AccountId,
			to: T::AccountId,
		},
		// runtime sends NewsFeed price set event when signed owner sets price
		NewsFeedPriceSet {
			newsfeed: UniqueId,
			subscribe_fee: Option<BalanceOf<T>>,
			unsubscribe_fee: Option<BalanceOf<T>>,
		},
		// runtime sends NewsFeed subscription paid event on successful payment
		NewsFeedSubscribeFeePaid {
			subscriber: T::AccountId,
			owner: T::AccountId,
			newsfeed: UniqueId,
			price: BalanceOf<T>,
		},
		// runtime sends NewsFeed unsubscribe fee paid event on successful payment
		NewsFeedUnsubscribeFeePaid {
			subscriber: T::AccountId,
			owner: T::AccountId,
			subscription: UniqueId,
			price: BalanceOf<T>,
		},
		// runtime sends NewsFeed unsubscribe fee paid event on successful payment
		NewsFeedNoUnsubscribeFee {
			subscriber: T::AccountId,
			subscription: UniqueId,
			newsfeed: UniqueId,
		},
		// runtime sends NewsFeed subscribed event on successful subscription
		NewsFeedSubscribed {
			newsfeed: UniqueId,
			subscriber: T::AccountId,
		},
		// runtime sends NewsFeed unsubscribed event on successful subscription cancellation
		NewsFeedUnsubscribed {
			newsfeed: UniqueId,
			subscriber: T::AccountId,
		},
	}

	// Prefix of the storage value used to communication with an offchain worker
	const ONCHAIN_TX_KEY: &[u8] = b"newsfeed::indexing";

	#[derive(Debug, Encode, Decode, Default)]
	#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
	struct IndexingUniqueId {
		id: Vec<u8>,
	}

	// Data to exchange with offchain worker
	#[derive(Debug, Encode, Decode, Default)]
	#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
	struct IndexingData(Vec<IndexingUniqueId>);

	// Callable functions
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		// Create a new/unique feed.
		#[pallet::weight(0)]
		pub fn create_newsfeed(
			origin: OriginFor<T>,
			payment: PaymentType,
			url: Vec<u8>,
		) -> DispatchResult {
			// Make sure the caller is from the signed origin
			let sender = ensure_signed(origin)?;

			// Generate unique id for new feed
			let newsfeed_id = Self::new_unique_id();

			// Min new feed
			Self::mint_newsfeed(&sender, newsfeed_id, payment, url.as_slice())?;

			Ok(())
		}

		// Transfer a newsfeed to another account.
		// Only signed owner of the newsfeed can do it
		#[pallet::weight(0)]
		pub fn transfer_newsfeed(
			origin: OriginFor<T>,
			to: T::AccountId,
			newsfeed_id: UniqueId,
		) -> DispatchResult {
			let from = ensure_signed(origin)?;
			let newsfeed = NewsFeeds::<T>::get(&newsfeed_id).ok_or(Error::<T>::NewsFeedNotFound)?;

			ensure!(from == newsfeed.owner, Error::<T>::NotNewsFeedOwner);

			Self::do_newsfeed_transfer(newsfeed_id, to)?;

			Ok(())
		}

		// Sets the price for a news feed.
		// Only signed owner is able to set a price.
		// If price is set to None nobody is able to subscribe to the news feed
		#[pallet::weight(0)]
		pub fn set_fees_newsfeed(
			origin: OriginFor<T>,
			newsfeed_id: UniqueId,
			subscribe_fee: Option<BalanceOf<T>>,
			unsubscribe_fee: Option<BalanceOf<T>>,
		) -> DispatchResult {
			// Make sure the caller is from the signed origin
			let caller = ensure_signed(origin)?;

			// ensure the news feed exists and is owned by the caller
			let mut newsfeed =
				NewsFeeds::<T>::get(&newsfeed_id).ok_or(Error::<T>::NewsFeedNotFound)?;
			ensure!(newsfeed.owner == caller, Error::<T>::NotNewsFeedOwner);

			newsfeed.subscribe_fee = subscribe_fee;
			newsfeed.unsubscribe_fee = unsubscribe_fee;
			NewsFeeds::<T>::insert(&newsfeed_id, newsfeed);

			Self::deposit_event(Event::<T>::NewsFeedPriceSet {
				newsfeed: newsfeed_id,
				subscribe_fee,
				unsubscribe_fee,
			});

			Ok(())
		}

		// Subscribe a news feed. If newsfeed is active (price is not None) a price is transfered from the caller account to the news feed owner account
		// We can't access offchain storage from the extrinsic, so for parallel processing we need to implement different approach:
		// 	Collect the data in a Vec<_> storage value on chain. You remove this storage at on_finalize and store it with sp_io::offchain_index::set
		#[pallet::weight(0)]
		pub fn subscribe_newsfeed(origin: OriginFor<T>, newsfeed_id: UniqueId) -> DispatchResult {
			// Ensure the caller is from a signed origin
			let subscriber = ensure_signed(origin)?;

			// Generate unique id for the subscription
			let subscription_id = Self::new_unique_id();

			// Subscribe to the news feed
			Self::do_subscribe(subscription_id, newsfeed_id, subscriber)?;

			// Add new subscription to the queue
			let mut queue = SubscriptionsPerBlock::<T>::get();
			queue
				.try_push(subscription_id)
				.map_err(|_| Error::<T>::MaxSubscriptionsPerBlock)?;

			SubscriptionsPerBlock::<T>::put(queue);

			Ok(())
		}

		// Cancells subscription to a news feed.
		#[pallet::weight(0)]
		pub fn unsubscribe_newsfeed(
			origin: OriginFor<T>,
			subscription_id: UniqueId,
		) -> DispatchResult {
			// Ensure the caller is from a signed origin
			let subscriber = ensure_signed(origin)?;

			Self::do_unsubscribe(subscription_id, subscriber)?;
			Ok(())
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_finalize(block_number: BlockNumberFor<T>) {
			log::info!("[News feed] block finalize: block number: {:?}", block_number);

			let subscriptions = SubscriptionsPerBlock::<T>::get();
			if !subscriptions.is_empty() {
				let storage_key = Self::dervied_key(block_number);
				let mut idx_data =
					IndexingData(Vec::<IndexingUniqueId>::with_capacity(subscriptions.len()));
				for id in subscriptions.iter() {
					idx_data.0.push(IndexingUniqueId { id: id.to_vec() });
				}

				SubscriptionsPerBlock::<T>::kill();

				offchain_index::set(&storage_key, &idx_data.encode());
			}
		}

		// Offchain worker entry point
		fn offchain_worker(block_number: T::BlockNumber) {
			log::info!("[News feed] offchain worker: block number: {:?}", block_number);

			let storage_key = Self::dervied_key(block_number);
			let storage_ref = StorageValueRef::persistent(&storage_key);
			if let Ok(Some(data)) = storage_ref.get::<IndexingData>() {
				for elem in data.0.iter() {
					log::info!(
						"[News feed] offchain worker, subscription to register: 0x{:?}",
						hexdisplay::HexDisplay::from(&elem.id)
					);
				}
			}
		}
	}

	// Pallet internal functions
	impl<T: Config> Pallet<T> {
		// calculate a unique for the current block key that can store a value for the offchain worker
		fn dervied_key(block_number: T::BlockNumber) -> Vec<u8> {
			block_number.using_encoded(|encoded_bn| {
				ONCHAIN_TX_KEY
					.clone()
					.into_iter()
					.chain(b"/".into_iter())
					.chain(encoded_bn)
					.copied()
					.collect::<Vec<u8>>()
			})
		}

		// generate unique id for a new feed
		fn new_unique_id() -> UniqueId {
			// create randomness
			let random = T::NewsFeedRandomness::random(&b"unique_id"[..]).0;
			// randomness payload enables that multiple feeds can be created in the same block
			let unique_payload = (
				random,
				frame_system::Pallet::<T>::extrinsic_index().unwrap_or_default(),
				frame_system::Pallet::<T>::block_number(),
			);
			let encoded_payload = unique_payload.encode();
			frame_support::Hashable::blake2_128(&encoded_payload)
		}

		// If the function needs ownership, you should pass by value. If the function only needs a reference, you should pass by reference.
		// Mint/create new feed. Minting means creating a new news feed on the blockchain.
		// UniqueId is a fixed size array of u8 so it automatically implements Copy and Clone
		// traits. So it is passed by value == copy.
		fn mint_newsfeed(
			owner: &T::AccountId,
			newsfeed_id: UniqueId,
			payment: PaymentType,
			url: &[u8],
		) -> DispatchResult {
			// create new feed
			let feed = NewsFeed::<T> {
				id: newsfeed_id,
				owner: owner.clone(),
				subscribe_fee: None,
				unsubscribe_fee: None,
				payment_type: payment,
				registration_url: BoundedVec::<u8, T::MaxRegistrationUrlLength>::try_from(
					url.to_vec(),
				)
				.map_err(|_| Error::<T>::RegistrationUrlTooLong)?,
			};

			// check that feed does not exist in the storage map
			ensure!(!NewsFeeds::<T>::contains_key(&feed.id), Error::<T>::DuplicatedNewsFeed);

			// check that a new feed does not exceed max available feeds
			let count = NewsFeedCount::<T>::get();
			let new_count = count.checked_add(1).ok_or(Error::<T>::NewsFeedsOverflow)?;
			ensure!(new_count <= T::MaxNewsFeeds::get(), Error::<T>::NewsFeedsOverflow);

			OwnerOfNewsFeeds::<T>::try_append(owner, newsfeed_id)
				.map_err(|_| Error::<T>::MaxNewsFeedsOwned)?;

			// write new feed to the storage and update the count
			NewsFeeds::<T>::insert(feed.id, feed);
			NewsFeedCount::<T>::put(new_count);

			// deposit the event
			Self::deposit_event(Event::<T>::NewsFeedCreated {
				newsfeed: newsfeed_id,
				owner: owner.clone(),
			});

			Ok(())
		}

		// Transfer news feed from a current owner to the new owner
		// - News feed must exists (must be registered)
		// - News feed can't be tranferred to its owner
		// - News feed can't be transferred to an account that already has the maximum news feeds
		// allowed
		fn do_newsfeed_transfer(newsfeed_id: UniqueId, to: T::AccountId) -> DispatchResult {
			// get the news feed. News feed must exist
			let mut newsfeed =
				NewsFeeds::<T>::get(&newsfeed_id).ok_or(Error::<T>::NewsFeedNotFound)?;

			// News feed can't be transferred to its owner
			let from = newsfeed.owner;
			ensure!(from != to, Error::<T>::TransferToSelf);

			// get news feeds owned by the from account
			let mut from_owned = OwnerOfNewsFeeds::<T>::get(&from);
			// remove news feed from the owned by the from account
			if let Some(ind) = from_owned.iter().position(|&id| id == newsfeed_id) {
				from_owned.swap_remove(ind);
			} else {
				return Err(Error::<T>::NewsFeedNotFound.into());
			}

			// add news feed to the vector owned by the `to` account
			let mut to_owned = OwnerOfNewsFeeds::<T>::get(&to);
			to_owned.try_push(newsfeed_id).map_err(|_| Error::<T>::MaxNewsFeedsOwned)?;

			// replace news feed owner with `to` account
			newsfeed.owner = to.clone();
			// set fees to None which means new subscriptions are not allowed
			newsfeed.subscribe_fee = None;
			newsfeed.unsubscribe_fee = None;

			// write updates to the storage
			NewsFeeds::<T>::insert(newsfeed_id, newsfeed);
			OwnerOfNewsFeeds::<T>::insert(&to, to_owned);
			OwnerOfNewsFeeds::<T>::insert(&from, from_owned);

			Self::deposit_event(Event::NewsFeedTransferred { newsfeed: newsfeed_id, from, to });

			Ok(())
		}

		fn do_subscribe(
			subscription_id: UniqueId,
			newsfeed_id: UniqueId,
			to: T::AccountId,
		) -> DispatchResult {
			// Ensure that the news feed exists
			let newsfeed = NewsFeeds::<T>::get(newsfeed_id).ok_or(Error::<T>::NewsFeedNotFound)?;

			// Get subscriptions owned by `to` account
			let mut to_owned = Subscriptions::<T>::get(to.clone());

			// Ensure that `to` account is not an owner of the news feed
			ensure!(newsfeed.owner != to, Error::<T>::SubscriptionNotAllowed);

			// Ensure that `to` account has not yet subscribed the news feed
			ensure!(
				!to_owned.iter().any(|s| s.newsfeed_id == newsfeed_id),
				Error::<T>::DuplicatedSubscription
			);

			// Create new subscription
			let subscription =
				Subscription::<T> { id: subscription_id, newsfeed_id, owner: to.clone() };
			// Try to push new subscription into the vector owned by the `to` account
			to_owned
				.try_push(subscription)
				.map_err(|_| Error::<T>::MaxSubscriptionsExceeded)?;

			// If news feed is not allowed for sale, return an error.
			// Otherwise transfer initial one-time payment
			if let Some(price) = newsfeed.subscribe_fee {
				// Transfer the amount from subscriber to the newsfeed owner
				T::Currency::transfer(
					&to,
					&newsfeed.owner,
					price,
					frame_support::traits::ExistenceRequirement::KeepAlive,
				)?;

				Self::deposit_event(Event::<T>::NewsFeedSubscribeFeePaid {
					subscriber: to.clone(),
					owner: newsfeed.owner,
					newsfeed: newsfeed.id,
					price,
				});
			} else {
				return Err(Error::<T>::SubscriptionNotAllowed.into());
			}

			// Write updates to the storage
			Subscriptions::<T>::insert(&to, to_owned);

			Self::deposit_event(Event::<T>::NewsFeedSubscribed {
				newsfeed: newsfeed_id,
				subscriber: to,
			});

			Ok(())
		}

		// Unsubscribes `from` account from the news feed.
		// Checks if subscription exists, that the `from` account is an owner of the subscription.
		// If news feed has an unsubscribe fee define, it transfer the fee to the news feed owner account.
		fn do_unsubscribe(subscription_id: UniqueId, from: T::AccountId) -> DispatchResult {
			// Get subscriptions owned by the `from` account
			let mut from_owned = Subscriptions::<T>::get(from.clone());

			// Find index of the subscription and remove it
			if let Some(ind) = from_owned.iter().position(|s| s.id == subscription_id) {
				let subscription = from_owned.get(ind).ok_or(Error::<T>::SubscriptionNotFound)?;
				let newsfeed = NewsFeeds::<T>::get(subscription.newsfeed_id)
					.ok_or(Error::<T>::NewsFeedNotFound)?;
				// if unsubscribe fee is defined, transfer it
				if let Some(fee) = newsfeed.unsubscribe_fee {
					// Transfer unsubscribe fee from the `from` account to news feed owner account
					T::Currency::transfer(
						&from,
						&newsfeed.owner,
						fee,
						frame_support::traits::ExistenceRequirement::KeepAlive,
					)?;

					Self::deposit_event(Event::<T>::NewsFeedUnsubscribeFeePaid {
						subscriber: from.clone(),
						owner: newsfeed.owner,
						subscription: subscription_id,
						price: fee,
					});
				} else {
					Self::deposit_event(Event::<T>::NewsFeedNoUnsubscribeFee {
						subscriber: from.clone(),
						subscription: subscription_id,
						newsfeed: newsfeed.id,
					})
				}
				from_owned.swap_remove(ind);

				// Write update to the storage
				Subscriptions::<T>::insert(&from, from_owned);

				Self::deposit_event(Event::<T>::NewsFeedUnsubscribed {
					newsfeed: newsfeed.id,
					subscriber: from,
				});
			} else {
				return Err(Error::<T>::SubscriptionNotFound.into());
			}

			Ok(())
		}
	}
}
